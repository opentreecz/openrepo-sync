use anyhow::{Context, Result, bail};
use std::path::Path;
use tracing::{debug, info, warn};

use crate::config::OnConflict;
use crate::config::ProjectConfig;
use crate::config::SourceConfig;
use crate::models::{RemotePackage, SyncAction, SyncResult};
use crate::repo_client::RepoClient;
use crate::sources::{
    direct_url::DirectUrlSource, github::GithubSource, sourceforge::SourceforgeSource,
};

pub async fn sync_project(
    project: &ProjectConfig,
    client: &RepoClient,
    download_dir: &Path,
    dry_run: bool,
) -> SyncResult {
    let mut result = SyncResult {
        project_name: project.name.clone(),
        actions: Vec::new(),
    };

    match sync_project_inner(project, client, download_dir, dry_run).await {
        Ok(actions) => result.actions = actions,
        Err(e) => {
            warn!("[{}] Error: {:#}", project.name, e);
            result.actions.push(SyncAction::Error(format!("{:#}", e)));
        }
    }
    result
}

async fn sync_project_inner(
    project: &ProjectConfig,
    client: &RepoClient,
    download_dir: &Path,
    dry_run: bool,
) -> Result<Vec<SyncAction>> {
    let mut actions = Vec::new();

    info!("[{}] Fetching upstream packages...", project.name);
    let remote_packages = fetch_upstream(project).await?;
    debug!(
        "[{}] Found {} upstream packages",
        project.name,
        remote_packages.len()
    );

    info!("[{}] Listing repository packages...", project.name);
    let mut repo_packages = client.list_packages(&project.repo_uid).await?;
    debug!(
        "[{}] Found {} repo packages",
        project.name,
        repo_packages.len()
    );

    // Find remote packages not already in the repo (by filename or version).
    // Version dedup is skipped for the raw "0" fallback to avoid false positives.
    let repo_filenames: std::collections::HashSet<_> =
        repo_packages.iter().map(|p| p.filename.as_str()).collect();
    let repo_versions: std::collections::HashSet<_> = repo_packages
        .iter()
        .map(|p| p.version.to_string())
        .collect();

    let to_upload: Vec<&RemotePackage> = remote_packages
        .iter()
        .filter(|p| {
            let version_str = p.version.to_string();
            !repo_filenames.contains(p.filename.as_str())
                && (version_str == "0" || !repo_versions.contains(&version_str))
        })
        .collect();

    if to_upload.is_empty() {
        info!("[{}] Up to date", project.name);
        actions.push(SyncAction::UpToDate);
    } else {
        for remote in &to_upload {
            info!(
                "[{}] Uploading {} ({})",
                project.name, remote.filename, remote.version
            );
            if !dry_run {
                let path = download_package(remote, download_dir).await?;
                let overwrite = project.on_conflict == OnConflict::Overwrite;
                let upload_result = client
                    .upload_package(&project.repo_uid, &path, overwrite)
                    .await;
                let _ = tokio::fs::remove_file(&path).await;
                match upload_result {
                    Ok(()) => {
                        actions.push(SyncAction::Uploaded {
                            version: remote.version.clone(),
                        });
                    }
                    Err(e)
                        if project.on_conflict == OnConflict::Skip
                            && e.to_string().contains("400") =>
                    {
                        info!(
                            "[{}] Skipping {} — already exists in repository",
                            project.name, remote.filename
                        );
                        actions.push(SyncAction::Skipped {
                            version: remote.version.clone(),
                        });
                    }
                    Err(e) => {
                        return Err(e)
                            .with_context(|| format!("Failed to upload {}", remote.filename));
                    }
                }
            } else {
                info!("[dry-run] Would upload {}", remote.filename);
                actions.push(SyncAction::Uploaded {
                    version: remote.version.clone(),
                });
            }
        }

        // Refresh repo package list after uploads
        if !dry_run {
            repo_packages = client.list_packages(&project.repo_uid).await?;
        }
    }

    // Prune: keep only the newest `keep_versions` packages
    repo_packages.sort_by(|a, b| b.version.cmp(&a.version));
    if repo_packages.len() > project.keep_versions {
        let to_delete = &repo_packages[project.keep_versions..];
        let count = to_delete.len();
        for pkg in to_delete {
            info!(
                "[{}] Pruning {} ({})",
                project.name, pkg.filename, pkg.version
            );
            if !dry_run {
                client
                    .delete_package(&project.repo_uid, &pkg.package_uid)
                    .await
                    .with_context(|| format!("Failed to delete {}", pkg.filename))?;
            } else {
                info!("[dry-run] Would delete {}", pkg.filename);
            }
        }
        actions.push(SyncAction::Pruned {
            removed_count: count,
        });
    }

    Ok(actions)
}

async fn fetch_upstream(project: &ProjectConfig) -> Result<Vec<RemotePackage>> {
    match &project.source {
        SourceConfig::Github {
            owner,
            repo,
            asset_filter,
            prerelease,
        } => {
            let source = GithubSource::new(owner, repo, asset_filter.as_deref(), *prerelease)?;
            source.fetch_latest(project.keep_versions).await
        }
        SourceConfig::DirectUrl { url } => {
            let source = DirectUrlSource::new(url, false)?;
            source.fetch_latest(1).await
        }
        SourceConfig::DirectUrlLatest { url } => {
            let source = DirectUrlSource::new(url, true)?;
            source.fetch_latest(1).await
        }
        SourceConfig::Sourceforge {
            project: sf_project,
            folder,
            filename_filter,
        } => {
            let source =
                SourceforgeSource::new(sf_project, folder.as_deref(), filename_filter.as_deref())?;
            source.fetch_latest(project.keep_versions).await
        }
    }
}

async fn download_package(
    remote: &RemotePackage,
    download_dir: &Path,
) -> Result<std::path::PathBuf> {
    // file:// URLs are already on disk (from DirectUrlLatest pre-download)
    if let Some(path_str) = remote.download_url.strip_prefix("file://") {
        let path = std::path::PathBuf::from(path_str);
        if !path.exists() {
            bail!(
                "Pre-downloaded package not found on disk: {}",
                path.display()
            );
        }
        return Ok(path);
    }

    tokio::fs::create_dir_all(download_dir)
        .await
        .context("Failed to create download directory")?;

    let dest = download_dir.join(&remote.filename);
    debug!("Downloading {} -> {}", remote.download_url, dest.display());

    let client = reqwest::Client::builder()
        .user_agent("openrepo-sync/0.1")
        .build()?;
    let resp = client
        .get(&remote.download_url)
        .send()
        .await
        .context("Download failed")?
        .error_for_status()
        .context("Download request error")?;

    let bytes = resp.bytes().await.context("Failed to read download body")?;
    tokio::fs::write(&dest, &bytes)
        .await
        .with_context(|| format!("Failed to write {}", dest.display()))?;

    Ok(dest)
}
