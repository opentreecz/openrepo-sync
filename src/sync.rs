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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::PackageVersion;
    use crate::test_util::{MockResponse, MockServer};

    fn project(url: &str, keep_versions: usize, on_conflict: OnConflict) -> ProjectConfig {
        ProjectConfig {
            name: "testproj".to_string(),
            repo_uid: "r".to_string(),
            keep_versions,
            on_conflict,
            source: SourceConfig::DirectUrl {
                url: url.to_string(),
            },
        }
    }

    fn empty_list() -> MockResponse {
        MockResponse::json(200, r#"{"results":[],"next":null}"#)
    }

    fn list_of(names: &[(&str, &str)]) -> MockResponse {
        let entries: Vec<String> = names
            .iter()
            .map(|(uid, name)| format!(r#"{{"package_uid":"{}","package_name":"{}"}}"#, uid, name))
            .collect();
        MockResponse::json(
            200,
            &format!(r#"{{"results":[{}],"next":null}}"#, entries.join(",")),
        )
    }

    // ── dry-run paths ──────────────────────────────────────────────────────

    #[tokio::test]
    async fn dry_run_new_package_reports_uploaded_without_requests() {
        let server = MockServer::start(vec![empty_list()]);
        let client = RepoClient::new(&server.url, "k").unwrap();
        let dir = tempfile::tempdir().unwrap();

        let p = project("https://example.com/tool-1.0.0.deb", 5, OnConflict::Error);
        let result = sync_project(&p, &client, dir.path(), true).await;

        assert_eq!(result.project_name, "testproj");
        assert_eq!(result.actions.len(), 1);
        assert!(matches!(
            &result.actions[0],
            SyncAction::Uploaded { version } if *version == PackageVersion::parse("1.0.0")
        ));
    }

    #[tokio::test]
    async fn already_present_filename_is_up_to_date() {
        let server = MockServer::start(vec![list_of(&[("u1", "tool-1.0.0.deb")])]);
        let client = RepoClient::new(&server.url, "k").unwrap();
        let dir = tempfile::tempdir().unwrap();

        let p = project("https://example.com/tool-1.0.0.deb", 5, OnConflict::Error);
        let result = sync_project(&p, &client, dir.path(), true).await;

        assert_eq!(result.actions.len(), 1);
        assert!(matches!(result.actions[0], SyncAction::UpToDate));
    }

    #[tokio::test]
    async fn same_version_different_filename_is_up_to_date() {
        // Repo has the same 1.0.0 under a different filename — version dedup.
        let server = MockServer::start(vec![list_of(&[("u1", "tool_1.0.0_amd64.deb")])]);
        let client = RepoClient::new(&server.url, "k").unwrap();
        let dir = tempfile::tempdir().unwrap();

        let p = project("https://example.com/tool-1.0.0.deb", 5, OnConflict::Error);
        let result = sync_project(&p, &client, dir.path(), true).await;

        assert!(matches!(result.actions[0], SyncAction::UpToDate));
    }

    #[tokio::test]
    async fn unversioned_packages_skip_version_dedup() {
        // Both repo and remote resolve to raw version "0": the version match
        // must NOT suppress the upload — only an identical filename would.
        let server = MockServer::start(vec![list_of(&[("u1", "noversion.deb")])]);
        let client = RepoClient::new(&server.url, "k").unwrap();
        let dir = tempfile::tempdir().unwrap();

        let p = project("https://example.com/other.deb", 5, OnConflict::Error);
        let result = sync_project(&p, &client, dir.path(), true).await;

        assert!(matches!(&result.actions[0], SyncAction::Uploaded { .. }));
    }

    #[tokio::test]
    async fn dry_run_prunes_beyond_keep_versions() {
        let server = MockServer::start(vec![list_of(&[
            ("u1", "tool-1.0.0.deb"),
            ("u2", "tool-2.0.0.deb"),
            ("u3", "tool-3.0.0.deb"),
        ])]);
        let client = RepoClient::new(&server.url, "k").unwrap();
        let dir = tempfile::tempdir().unwrap();

        // Remote 3.0.0 already present → UpToDate, then prune down to 1.
        let p = project("https://example.com/tool-3.0.0.deb", 1, OnConflict::Error);
        let result = sync_project(&p, &client, dir.path(), true).await;

        assert_eq!(result.actions.len(), 2);
        assert!(matches!(result.actions[0], SyncAction::UpToDate));
        assert!(matches!(
            result.actions[1],
            SyncAction::Pruned { removed_count: 2 }
        ));
    }

    // ── real upload path via file:// package ───────────────────────────────

    #[tokio::test]
    async fn uploads_local_package_and_reports_uploaded() {
        let staging = tempfile::tempdir().unwrap();
        let pkg_path = staging.path().join("tool-1.2.0.deb");
        std::fs::write(&pkg_path, b"fake-deb").unwrap();

        let server = MockServer::start(vec![
            empty_list(),                  // initial repo listing
            MockResponse::json(200, "{}"), // upload
            empty_list(),                  // refresh listing after upload
        ]);
        let client = RepoClient::new(&server.url, "k").unwrap();
        let dir = tempfile::tempdir().unwrap();

        let p = project(
            &format!("file://{}", pkg_path.display()),
            5,
            OnConflict::Error,
        );
        let result = sync_project(&p, &client, dir.path(), false).await;

        assert_eq!(result.actions.len(), 1);
        assert!(matches!(
            &result.actions[0],
            SyncAction::Uploaded { version } if *version == PackageVersion::parse("1.2.0")
        ));
        // The uploaded file is cleaned up afterwards.
        assert!(!pkg_path.exists());

        let requests = server.requests();
        assert!(requests[1].starts_with("POST /api/r/upload/"));
    }

    #[tokio::test]
    async fn conflict_with_skip_policy_reports_skipped() {
        let staging = tempfile::tempdir().unwrap();
        let pkg_path = staging.path().join("tool-1.2.0.deb");
        std::fs::write(&pkg_path, b"fake-deb").unwrap();

        let server = MockServer::start(vec![
            empty_list(),
            MockResponse::json(400, "already exists"), // upload rejected
            empty_list(),
        ]);
        let client = RepoClient::new(&server.url, "k").unwrap();
        let dir = tempfile::tempdir().unwrap();

        let p = project(
            &format!("file://{}", pkg_path.display()),
            5,
            OnConflict::Skip,
        );
        let result = sync_project(&p, &client, dir.path(), false).await;

        assert_eq!(result.actions.len(), 1);
        assert!(matches!(
            &result.actions[0],
            SyncAction::Skipped { version } if *version == PackageVersion::parse("1.2.0")
        ));
    }

    #[tokio::test]
    async fn conflict_with_error_policy_reports_error() {
        let staging = tempfile::tempdir().unwrap();
        let pkg_path = staging.path().join("tool-1.2.0.deb");
        std::fs::write(&pkg_path, b"fake-deb").unwrap();

        let server = MockServer::start(vec![
            empty_list(),
            MockResponse::json(400, "already exists"),
        ]);
        let client = RepoClient::new(&server.url, "k").unwrap();
        let dir = tempfile::tempdir().unwrap();

        let p = project(
            &format!("file://{}", pkg_path.display()),
            5,
            OnConflict::Error,
        );
        let result = sync_project(&p, &client, dir.path(), false).await;

        assert_eq!(result.actions.len(), 1);
        match &result.actions[0] {
            SyncAction::Error(msg) => {
                assert!(msg.contains("Failed to upload"), "unexpected: {}", msg)
            }
            other => panic!("expected Error action, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn listing_failure_becomes_error_action() {
        let server = MockServer::start(vec![MockResponse::json(500, "boom")]);
        let client = RepoClient::new(&server.url, "k").unwrap();
        let dir = tempfile::tempdir().unwrap();

        let p = project("https://example.com/tool-1.0.0.deb", 5, OnConflict::Error);
        let result = sync_project(&p, &client, dir.path(), true).await;

        assert!(matches!(&result.actions[0], SyncAction::Error(_)));
    }

    // ── download_package ───────────────────────────────────────────────────

    #[tokio::test]
    async fn download_package_accepts_existing_file_url() {
        let staging = tempfile::tempdir().unwrap();
        let pkg_path = staging.path().join("tool-1.0.0.deb");
        std::fs::write(&pkg_path, b"bytes").unwrap();

        let remote = RemotePackage {
            filename: "tool-1.0.0.deb".to_string(),
            version: PackageVersion::parse("1.0.0"),
            download_url: format!("file://{}", pkg_path.display()),
        };
        let dir = tempfile::tempdir().unwrap();
        let path = download_package(&remote, dir.path()).await.unwrap();
        assert_eq!(path, pkg_path);
    }

    #[tokio::test]
    async fn download_package_rejects_missing_file_url() {
        let remote = RemotePackage {
            filename: "gone.deb".to_string(),
            version: PackageVersion::parse("1.0.0"),
            download_url: "file:///nonexistent/gone.deb".to_string(),
        };
        let dir = tempfile::tempdir().unwrap();
        let err = download_package(&remote, dir.path()).await.unwrap_err();
        assert!(err.to_string().contains("not found on disk"));
    }

    #[tokio::test]
    async fn download_package_fetches_http_url_to_download_dir() {
        let server = MockServer::start(vec![MockResponse::json(200, "deb-bytes")]);
        let remote = RemotePackage {
            filename: "tool-1.0.0.deb".to_string(),
            version: PackageVersion::parse("1.0.0"),
            download_url: format!("{}/tool-1.0.0.deb", server.url),
        };
        let dir = tempfile::tempdir().unwrap();
        let path = download_package(&remote, dir.path()).await.unwrap();
        assert_eq!(path, dir.path().join("tool-1.0.0.deb"));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "deb-bytes");
    }

    #[tokio::test]
    async fn download_package_http_error_fails() {
        let server = MockServer::start(vec![MockResponse::json(404, "nope")]);
        let remote = RemotePackage {
            filename: "gone.deb".to_string(),
            version: PackageVersion::parse("1.0.0"),
            download_url: format!("{}/gone.deb", server.url),
        };
        let dir = tempfile::tempdir().unwrap();
        let err = download_package(&remote, dir.path()).await.unwrap_err();
        assert!(err.to_string().contains("Download request error"));
    }
}
