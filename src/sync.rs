use anyhow::{Context, Result, bail};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashSet};
use std::path::Path;
use tracing::{debug, info, warn};

use crate::config::OnConflict;
use crate::config::ProjectConfig;
use crate::config::SourceConfig;
use crate::errors::UploadError;
use crate::models::{RemotePackage, SyncAction, SyncResult};
use crate::repo_client::RepoClient;
use crate::sources::{
    deb_repo::DebRepoSource, direct_url::DirectUrlSource, github::GithubSource,
    rpm_repo::RpmRepoSource, sourceforge::SourceforgeSource,
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
                    Err(UploadError::PackageExists)
                        if project.on_conflict == OnConflict::Skip =>
                    {
                        info!(
                            "[{}] Skipping {} — already exists in repository",
                            project.name, remote.filename
                        );
                        actions.push(SyncAction::Skipped {
                            version: remote.version.clone(),
                        });
                    }
                    Err(UploadError::PackageExists) => {
                        return Err(anyhow::anyhow!(
                            "Package {} already exists in repository (use on_conflict: skip or overwrite)",
                            remote.filename
                        ))
                        .with_context(|| format!("Failed to upload {}", remote.filename));
                    }
                    Err(UploadError::Other(e)) => {
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

    let managed_groups = managed_groups(&remote_packages, &repo_packages);
    let to_delete = prune_candidates(&repo_packages, &managed_groups, project.keep_versions);
    if !to_delete.is_empty() {
        let count = to_delete.len();
        for pkg in &to_delete {
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

fn managed_groups(
    remote_packages: &[RemotePackage],
    repo_packages: &[crate::models::RepoPackage],
) -> HashSet<(String, String)> {
    let remote_filenames: HashSet<&str> = remote_packages
        .iter()
        .map(|pkg| pkg.filename.as_str())
        .collect();
    let mut groups = HashSet::new();

    for remote in remote_packages {
        if let (Some(name), Some(arch)) = (&remote.package_name, &remote.architecture) {
            groups.insert((name.clone(), arch.clone()));
        }
    }

    for repo_pkg in repo_packages {
        if remote_filenames.contains(repo_pkg.filename.as_str()) {
            groups.insert((repo_pkg.package_name.clone(), repo_pkg.architecture.clone()));
        }
    }

    groups
}

fn prune_candidates(
    repo_packages: &[crate::models::RepoPackage],
    managed_groups: &HashSet<(String, String)>,
    keep_versions: usize,
) -> Vec<crate::models::RepoPackage> {
    let mut grouped: BTreeMap<(String, String), Vec<crate::models::RepoPackage>> = BTreeMap::new();

    for pkg in repo_packages {
        let key = (pkg.package_name.clone(), pkg.architecture.clone());
        if managed_groups.contains(&key) {
            grouped.entry(key).or_default().push(pkg.clone());
        }
    }

    let mut to_delete = Vec::new();
    for (_, mut group) in grouped {
        group.sort_by(|a, b| b.version.cmp(&a.version));
        if group.len() > keep_versions {
            to_delete.extend(group.into_iter().skip(keep_versions));
        }
    }
    to_delete
}

async fn fetch_upstream(project: &ProjectConfig) -> Result<Vec<RemotePackage>> {
    match &project.source {
        SourceConfig::Github {
            owner,
            repo,
            asset_filter,
            prerelease,
            arch_filter,
        } => {
            let source = GithubSource::new(
                owner,
                repo,
                asset_filter.as_deref(),
                *prerelease,
                arch_filter.clone(),
            )?;
            source.fetch_latest(project.keep_versions).await
        }
        SourceConfig::DirectUrl { url, sha256 } => {
            let source = DirectUrlSource::new(url, false, sha256.as_deref())?;
            source.fetch_latest(1).await
        }
        SourceConfig::DirectUrlLatest { url, sha256 } => {
            let source = DirectUrlSource::new(url, true, sha256.as_deref())?;
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
        SourceConfig::DebRepo {
            url,
            layout,
            suites,
            components,
            architectures,
            package_filter,
            filename_filter,
            verify_gpg,
            gpg_key,
        } => {
            let source = DebRepoSource::new(
                url,
                layout.clone(),
                suites.clone(),
                components.clone(),
                architectures.clone(),
                package_filter.clone(),
                filename_filter.as_deref(),
                *verify_gpg,
                gpg_key.as_deref(),
            )?;
            source.fetch_latest(project.keep_versions).await
        }
        SourceConfig::RpmRepo {
            url,
            package_filter,
            filename_filter,
            verify_gpg,
            gpg_key,
            architectures,
        } => {
            let source = RpmRepoSource::new(
                url,
                package_filter.clone(),
                filename_filter.as_deref(),
                *verify_gpg,
                gpg_key.as_deref(),
                architectures.clone(),
            )?;
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
        verify_sha256_path(&path, remote.sha256.as_deref())?;
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
    verify_sha256_bytes(bytes.as_ref(), remote.sha256.as_deref())?;
    tokio::fs::write(&dest, &bytes)
        .await
        .with_context(|| format!("Failed to write {}", dest.display()))?;

    Ok(dest)
}

fn verify_sha256_path(path: &Path, expected: Option<&str>) -> Result<()> {
    if let Some(expected) = expected {
        let bytes = std::fs::read(path).with_context(|| {
            format!("Failed to read {} for SHA-256 verification", path.display())
        })?;
        verify_sha256_bytes(&bytes, Some(expected))
            .with_context(|| format!("SHA-256 verification failed for {}", path.display()))?;
    }
    Ok(())
}

fn verify_sha256_bytes(bytes: &[u8], expected: Option<&str>) -> Result<()> {
    if let Some(expected) = expected {
        let actual = format!("{:x}", Sha256::digest(bytes));
        let expected = expected.trim().to_ascii_lowercase();
        if actual != expected {
            bail!("SHA-256 mismatch: expected {}, got {}", expected, actual);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::PackageVersion;
    use crate::test_util::{MockResponse, MockServer};
    use sha2::{Digest, Sha256};

    fn project(url: &str, keep_versions: usize, on_conflict: OnConflict) -> ProjectConfig {
        ProjectConfig {
            name: "testproj".to_string(),
            repo_uid: "r".to_string(),
            keep_versions,
            on_conflict,
            source: SourceConfig::DirectUrl {
                url: url.to_string(),
                sha256: None,
            },
        }
    }

    fn empty_list() -> MockResponse {
        MockResponse::json(200, r#"{"results":[],"next":null}"#)
    }

    fn list_repo_packages(entries: &[(&str, &str, &str, &str)]) -> MockResponse {
        let entries: Vec<String> = entries
            .iter()
            .map(|(uid, package_name, architecture, filename)| {
                let version = crate::version::extract_version_from_filename(filename)
                    .map(|v| v.to_string())
                    .unwrap_or_else(|| "0".to_string());
                format!(
                    r#"{{"package_uid":"{}","package_name":"{}","filename":"{}","architecture":"{}","version":"{}"}}"#,
                    uid, package_name, filename, architecture, version
                )
            })
            .collect();
        MockResponse::json(
            200,
            &format!(r#"{{"results":[{}],"next":null}}"#, entries.join(",")),
        )
    }

    fn list_of(names: &[(&str, &str)]) -> MockResponse {
        let entries: Vec<_> = names
            .iter()
            .map(|(uid, name)| (*uid, "tool", "amd64", *name))
            .collect();
        list_repo_packages(&entries)
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

    #[test]
    fn prune_candidates_only_touch_managed_package_arch_groups() {
        let repo_packages = vec![
            crate::models::RepoPackage {
                package_uid: "u1".to_string(),
                filename: "tool-3.0.0-amd64.deb".to_string(),
                package_name: "tool".to_string(),
                architecture: "amd64".to_string(),
                version: PackageVersion::parse("3.0.0"),
            },
            crate::models::RepoPackage {
                package_uid: "u2".to_string(),
                filename: "tool-2.0.0-amd64.deb".to_string(),
                package_name: "tool".to_string(),
                architecture: "amd64".to_string(),
                version: PackageVersion::parse("2.0.0"),
            },
            crate::models::RepoPackage {
                package_uid: "u3".to_string(),
                filename: "tool-1.0.0-arm64.deb".to_string(),
                package_name: "tool".to_string(),
                architecture: "arm64".to_string(),
                version: PackageVersion::parse("1.0.0"),
            },
            crate::models::RepoPackage {
                package_uid: "u4".to_string(),
                filename: "manual-1.0.0.deb".to_string(),
                package_name: "manual".to_string(),
                architecture: "amd64".to_string(),
                version: PackageVersion::parse("1.0.0"),
            },
        ];
        let managed = HashSet::from([(String::from("tool"), String::from("amd64"))]);

        let to_delete = prune_candidates(&repo_packages, &managed, 1);

        assert_eq!(to_delete.len(), 1);
        assert_eq!(to_delete[0].package_uid, "u2");
    }

    #[test]
    fn managed_groups_use_remote_metadata_when_available() {
        let remote_packages = vec![RemotePackage {
            filename: "tool-1.0.0-amd64.deb".to_string(),
            version: PackageVersion::parse("1.0.0"),
            download_url: "https://example.com/tool-1.0.0-amd64.deb".to_string(),
            sha256: None,
            package_name: Some("tool".to_string()),
            architecture: Some("amd64".to_string()),
        }];

        let groups = managed_groups(&remote_packages, &[]);

        assert!(groups.contains(&(String::from("tool"), String::from("amd64"))));
    }

    #[tokio::test]
    async fn sync_project_delete_requests_only_target_managed_group() {
        let server = MockServer::start(vec![
            list_repo_packages(&[
                ("keep-new", "tool", "amd64", "tool_3.0.0_amd64.deb"),
                ("keep-mid", "tool", "amd64", "tool_2.0.0_amd64.deb"),
                ("drop-old", "tool", "amd64", "tool_1.0.0_amd64.deb"),
                ("arm-keep", "tool", "arm64", "tool_3.0.0_arm64.deb"),
                (
                    "manual",
                    "manual-package",
                    "amd64",
                    "manual_9.9.9_amd64.deb",
                ),
                (
                    "other-project",
                    "other-tool",
                    "amd64",
                    "other_5.0.0_amd64.deb",
                ),
            ]),
            MockResponse::json(200, "{}"),
        ]);
        let client = RepoClient::new(&server.url, "k").unwrap();
        let dir = tempfile::tempdir().unwrap();

        let p = project(
            "https://example.com/tool_3.0.0_amd64.deb",
            2,
            OnConflict::Error,
        );
        let result = sync_project(&p, &client, dir.path(), false).await;

        assert!(matches!(result.actions[0], SyncAction::UpToDate));
        assert!(matches!(
            result.actions[1],
            SyncAction::Pruned { removed_count: 1 }
        ));

        let requests = server.requests();
        assert_eq!(requests.len(), 2);
        assert!(requests[1].starts_with("DELETE /api/r/pkg/drop-old/"));
    }

    #[test]
    fn prune_candidates_enforces_keep_versions_per_architecture() {
        let remote_packages = vec![
            RemotePackage {
                filename: "tool_3.0.0_amd64.deb".to_string(),
                version: PackageVersion::parse("3.0.0"),
                download_url: "https://example.com/tool_3.0.0_amd64.deb".to_string(),
                sha256: None,
                package_name: Some("tool".to_string()),
                architecture: Some("amd64".to_string()),
            },
            RemotePackage {
                filename: "tool_3.0.0_arm64.deb".to_string(),
                version: PackageVersion::parse("3.0.0"),
                download_url: "https://example.com/tool_3.0.0_arm64.deb".to_string(),
                sha256: None,
                package_name: Some("tool".to_string()),
                architecture: Some("arm64".to_string()),
            },
        ];
        let initial_packages = vec![
            crate::models::RepoPackage {
                package_uid: "amd64-new".to_string(),
                filename: "tool_3.0.0_amd64.deb".to_string(),
                package_name: "tool".to_string(),
                architecture: "amd64".to_string(),
                version: PackageVersion::parse("3.0.0"),
            },
            crate::models::RepoPackage {
                package_uid: "amd64-mid".to_string(),
                filename: "tool_2.0.0_amd64.deb".to_string(),
                package_name: "tool".to_string(),
                architecture: "amd64".to_string(),
                version: PackageVersion::parse("2.0.0"),
            },
            crate::models::RepoPackage {
                package_uid: "amd64-old".to_string(),
                filename: "tool_1.0.0_amd64.deb".to_string(),
                package_name: "tool".to_string(),
                architecture: "amd64".to_string(),
                version: PackageVersion::parse("1.0.0"),
            },
            crate::models::RepoPackage {
                package_uid: "arm64-new".to_string(),
                filename: "tool_3.0.0_arm64.deb".to_string(),
                package_name: "tool".to_string(),
                architecture: "arm64".to_string(),
                version: PackageVersion::parse("3.0.0"),
            },
            crate::models::RepoPackage {
                package_uid: "arm64-mid".to_string(),
                filename: "tool_2.0.0_arm64.deb".to_string(),
                package_name: "tool".to_string(),
                architecture: "arm64".to_string(),
                version: PackageVersion::parse("2.0.0"),
            },
            crate::models::RepoPackage {
                package_uid: "arm64-old".to_string(),
                filename: "tool_1.0.0_arm64.deb".to_string(),
                package_name: "tool".to_string(),
                architecture: "arm64".to_string(),
                version: PackageVersion::parse("1.0.0"),
            },
        ];

        let managed = managed_groups(&remote_packages, &initial_packages);
        let to_delete = prune_candidates(&initial_packages, &managed, 2);

        assert_eq!(to_delete.len(), 2);
        assert!(to_delete.iter().any(|pkg| pkg.package_uid == "amd64-old"));
        assert!(to_delete.iter().any(|pkg| pkg.package_uid == "arm64-old"));
    }

    // ── real upload path via file:// package ───────────────────────────────

    #[tokio::test]
    async fn uploads_local_package_and_reports_uploaded() {
        let staging = tempfile::tempdir().unwrap();
        let pkg_path = staging.path().join("tool-1.2.0.deb");
        std::fs::write(&pkg_path, b"fake-deb").unwrap();

        let server = MockServer::start(vec![
            empty_list(),                                   // initial repo listing
            MockResponse::json(202, r#"{"task_id":"t1"}"#), // upload accepted
            MockResponse::json(200, r#"{"status":"completed","error_message":""}"#), // status poll
            empty_list(),                                   // refresh listing after upload
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

        // Server accepts upload (202) then background processing fails with
        // "already exists" — the status poll returns "failed".
        let server = MockServer::start(vec![
            empty_list(),
            MockResponse::json(202, r#"{"task_id":"skip-t"}"#),
            MockResponse::json(
                200,
                r#"{"status":"failed","error_message":"Package tool already exists in destination repo r and 'overwrite' is not specified"}"#,
            ),
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
    async fn conflict_with_skip_policy_http409_reports_skipped() {
        // Phase 1.3+ path: server returns 409 Conflict directly (synchronous).
        let staging = tempfile::tempdir().unwrap();
        let pkg_path = staging.path().join("tool-1.2.0.deb");
        std::fs::write(&pkg_path, b"fake-deb").unwrap();

        let server = MockServer::start(vec![
            empty_list(),
            MockResponse::json(
                409,
                r#"{"code":"PACKAGE_EXISTS","detail":"already exists","status":409}"#,
            ),
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
            MockResponse::json(202, r#"{"task_id":"err-t"}"#),
            MockResponse::json(
                200,
                r#"{"status":"failed","error_message":"Package tool already exists in destination repo r and 'overwrite' is not specified"}"#,
            ),
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

    #[tokio::test]
    async fn listing_404_becomes_error_action() {
        // Regression: 404 on listing must NOT silently return empty — it
        // must be reported as an error to prevent duplicate uploads.
        let server = MockServer::start(vec![MockResponse::json(404, "{}")]);
        let client = RepoClient::new(&server.url, "k").unwrap();
        let dir = tempfile::tempdir().unwrap();

        let p = project("https://example.com/tool-1.0.0.deb", 5, OnConflict::Error);
        let result = sync_project(&p, &client, dir.path(), true).await;

        assert!(matches!(&result.actions[0], SyncAction::Error(_)));
        if let SyncAction::Error(msg) = &result.actions[0] {
            assert!(
                msg.contains("404"),
                "expected error to mention 404, got: {}",
                msg
            );
        }
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
            sha256: None,
            package_name: None,
            architecture: None,
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
            sha256: None,
            package_name: None,
            architecture: None,
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
            sha256: None,
            package_name: None,
            architecture: None,
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
            sha256: None,
            package_name: None,
            architecture: None,
        };
        let dir = tempfile::tempdir().unwrap();
        let err = download_package(&remote, dir.path()).await.unwrap_err();
        assert!(err.to_string().contains("Download request error"));
    }

    #[tokio::test]
    async fn download_package_verifies_http_sha256() {
        let body = b"deb-bytes".to_vec();
        let sha256 = format!("{:x}", Sha256::digest(&body));
        let server = MockServer::start(vec![MockResponse::bytes(200, body, &[])]);
        let remote = RemotePackage {
            filename: "tool-1.0.0.deb".to_string(),
            version: PackageVersion::parse("1.0.0"),
            download_url: format!("{}/tool-1.0.0.deb", server.url),
            sha256: Some(sha256),
            package_name: None,
            architecture: None,
        };
        let dir = tempfile::tempdir().unwrap();
        let path = download_package(&remote, dir.path()).await.unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"deb-bytes");
    }

    #[tokio::test]
    async fn download_package_rejects_sha256_mismatch() {
        let server = MockServer::start(vec![MockResponse::bytes(200, b"deb-bytes".to_vec(), &[])]);
        let remote = RemotePackage {
            filename: "tool-1.0.0.deb".to_string(),
            version: PackageVersion::parse("1.0.0"),
            download_url: format!("{}/tool-1.0.0.deb", server.url),
            sha256: Some(
                "0000000000000000000000000000000000000000000000000000000000000000".to_string(),
            ),
            package_name: None,
            architecture: None,
        };
        let dir = tempfile::tempdir().unwrap();
        let err = download_package(&remote, dir.path()).await.unwrap_err();
        assert!(err.to_string().contains("SHA-256 mismatch"));
    }

    #[tokio::test]
    async fn download_package_verifies_file_sha256() {
        let staging = tempfile::tempdir().unwrap();
        let pkg_path = staging.path().join("tool-1.0.0.deb");
        std::fs::write(&pkg_path, b"bytes").unwrap();
        let sha256 = format!("{:x}", Sha256::digest(b"bytes"));

        let remote = RemotePackage {
            filename: "tool-1.0.0.deb".to_string(),
            version: PackageVersion::parse("1.0.0"),
            download_url: format!("file://{}", pkg_path.display()),
            sha256: Some(sha256),
            package_name: None,
            architecture: None,
        };
        let dir = tempfile::tempdir().unwrap();
        let path = download_package(&remote, dir.path()).await.unwrap();
        assert_eq!(path, pkg_path);
    }
}
