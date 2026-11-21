// SPDX-License-Identifier: AGPL-3.0-only
//! Cleanup of leftover local files and the OpenRepo server's storage.
//!
//! * **Local**: packages are downloaded to `download_dir` (or staged in
//!   [`staging_dir`] by `direct_url_latest`) and deleted after upload. A crash,
//!   a killed container or a failed upload can leave them behind; the sweep
//!   removes top-level regular files older than `cleanup.local_min_age` at the
//!   start of every run.
//! * **Server**: after a sync pass, optionally call `POST /api/cleanup/` so the
//!   server garbage-collects unused package files, abandoned uploads, stale
//!   builds and publish directories (`cleanup.server: report | apply`).

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use tracing::{debug, info, warn};

use crate::config::ServerCleanup;
use crate::repo_client::RepoClient;

/// Directory where `direct_url_latest` stages downloaded packages.
pub fn staging_dir() -> PathBuf {
    std::env::temp_dir().join("openrepo-sync-latest")
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct LocalCleanup {
    pub files: u64,
    pub bytes: u64,
}

/// Directories that must never be swept even if configured as `download_dir`.
fn is_protected(dir: &Path) -> bool {
    let Ok(dir) = dir.canonicalize() else {
        return false;
    };
    let mut protected = vec![PathBuf::from("/")];
    if let Ok(tmp) = std::env::temp_dir().canonicalize() {
        protected.push(tmp);
    }
    if let Some(home) = std::env::var_os("HOME").and_then(|h| PathBuf::from(h).canonicalize().ok())
    {
        protected.push(home);
    }
    protected.contains(&dir)
}

/// Remove top-level regular files older than `min_age` from each directory.
/// Sub-directories and symlinks are never touched.
pub fn sweep_local(dirs: &[PathBuf], min_age: Duration, dry_run: bool) -> LocalCleanup {
    let mut result = LocalCleanup::default();
    let now = SystemTime::now();

    for dir in dirs {
        if !dir.is_dir() {
            continue;
        }
        if is_protected(dir) {
            warn!(
                "Skipping local cleanup of {}: not a dedicated download directory",
                dir.display()
            );
            continue;
        }
        let entries = match std::fs::read_dir(dir) {
            Ok(entries) => entries,
            Err(e) => {
                warn!("Cannot read {} for cleanup: {}", dir.display(), e);
                continue;
            }
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(meta) = std::fs::symlink_metadata(&path) else {
                continue;
            };
            if !meta.file_type().is_file() {
                continue;
            }
            let age = meta
                .modified()
                .ok()
                .and_then(|m| now.duration_since(m).ok())
                .unwrap_or_default();
            if age < min_age {
                continue;
            }
            if dry_run {
                info!("[dry-run] Would remove leftover {}", path.display());
            } else if let Err(e) = std::fs::remove_file(&path) {
                warn!("Failed to remove leftover {}: {}", path.display(), e);
                continue;
            } else {
                debug!("Removed leftover {}", path.display());
            }
            result.files += 1;
            result.bytes += meta.len();
        }
    }
    result
}

/// Ask the OpenRepo server to clean up. Failures are logged, never fatal.
pub async fn run_server_cleanup(client: &RepoClient, mode: ServerCleanup, dry_run: bool) {
    let server_dry_run = match mode {
        ServerCleanup::Off => return,
        ServerCleanup::Report => true,
        ServerCleanup::Apply => dry_run,
    };
    match client.cleanup(server_dry_run).await {
        Ok(r) => {
            let verb = if r.dry_run { "would remove" } else { "removed" };
            info!(
                "Server cleanup{}: {} {} orphan file(s) ({:.2} GB), {} stale publish dir(s); \
                 abandoned uploads={}, stale builds={}, old upload tasks={}, old builds={}",
                if r.dry_run { " [dry-run]" } else { "" },
                verb,
                r.orphan_files,
                r.orphan_bytes as f64 / 1e9,
                r.stale_publish_dirs.len(),
                r.abandoned_uploads,
                r.stale_builds,
                r.old_upload_tasks,
                r.old_builds,
            );
            if !r.unmanaged_publish_dirs.is_empty() {
                info!(
                    "Server keeps publish dirs of deleted repos: {}",
                    r.unmanaged_publish_dirs.join(", ")
                );
            }
            for e in &r.errors {
                warn!("Server cleanup error: {}", e);
            }
        }
        Err(e) => warn!("Server cleanup skipped: {:#}", e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::{MockResponse, MockServer, test_client};

    fn age_file(path: &Path, secs: u64) {
        let t = SystemTime::now() - Duration::from_secs(secs);
        std::fs::File::options()
            .write(true)
            .open(path)
            .unwrap()
            .set_modified(t)
            .unwrap();
    }

    #[test]
    fn sweep_removes_only_old_top_level_files() {
        let dir = tempfile::tempdir().unwrap();
        let old = dir.path().join("old.deb");
        let young = dir.path().join("young.deb");
        let sub = dir.path().join("sub");
        std::fs::write(&old, b"12345").unwrap();
        std::fs::write(&young, b"x").unwrap();
        std::fs::create_dir(&sub).unwrap();
        std::fs::write(sub.join("nested.deb"), b"x").unwrap();
        age_file(&old, 7200);
        age_file(&sub.join("nested.deb"), 7200);

        let r = sweep_local(
            &[dir.path().to_path_buf()],
            Duration::from_secs(3600),
            false,
        );

        assert_eq!(r, LocalCleanup { files: 1, bytes: 5 });
        assert!(!old.exists());
        assert!(young.exists());
        assert!(sub.join("nested.deb").exists());
    }

    #[test]
    fn sweep_dry_run_keeps_files() {
        let dir = tempfile::tempdir().unwrap();
        let old = dir.path().join("old.deb");
        std::fs::write(&old, b"x").unwrap();
        age_file(&old, 7200);
        let r = sweep_local(&[dir.path().to_path_buf()], Duration::from_secs(60), true);
        assert_eq!(r.files, 1);
        assert!(old.exists());
    }

    #[cfg(unix)]
    #[test]
    fn sweep_ignores_symlinks() {
        let dir = tempfile::tempdir().unwrap();
        let target = tempfile::NamedTempFile::new().unwrap();
        age_file(target.path(), 7200);
        std::os::unix::fs::symlink(target.path(), dir.path().join("link.deb")).unwrap();
        let r = sweep_local(&[dir.path().to_path_buf()], Duration::ZERO, false);
        assert_eq!(r.files, 0);
        assert!(target.path().exists());
    }

    #[test]
    fn sweep_skips_missing_and_protected_dirs() {
        let r = sweep_local(
            &[
                PathBuf::from("/nonexistent/openrepo-sync"),
                std::env::temp_dir(),
                PathBuf::from("/"),
            ],
            Duration::ZERO,
            true,
        );
        assert_eq!(r.files, 0);
    }

    #[tokio::test]
    async fn server_cleanup_off_makes_no_request() {
        let server = MockServer::start(vec![]);
        let client = RepoClient::new(&server.url, "k", test_client()).unwrap();
        run_server_cleanup(&client, ServerCleanup::Off, false).await;
        assert!(server.requests().is_empty());
    }

    #[tokio::test]
    async fn server_cleanup_report_sends_dry_run_even_without_cli_dry_run() {
        let server = MockServer::start(vec![MockResponse::json(
            200,
            r#"{"dry_run": true, "orphan_files": 3, "orphan_bytes": 1000}"#,
        )]);
        let client = RepoClient::new(&server.url, "k", test_client()).unwrap();
        run_server_cleanup(&client, ServerCleanup::Report, false).await;
        let reqs = server.requests();
        assert!(reqs[0].starts_with("POST /api/cleanup/"), "{}", reqs[0]);
        assert!(reqs[0].contains(r#""dry_run":true"#), "{}", reqs[0]);
    }

    #[tokio::test]
    async fn server_cleanup_apply_respects_cli_dry_run() {
        let server = MockServer::start(vec![
            MockResponse::json(200, r#"{"dry_run": false}"#),
            MockResponse::json(200, r#"{"dry_run": true}"#),
        ]);
        let client = RepoClient::new(&server.url, "k", test_client()).unwrap();
        run_server_cleanup(&client, ServerCleanup::Apply, false).await;
        run_server_cleanup(&client, ServerCleanup::Apply, true).await;
        let reqs = server.requests();
        assert!(reqs[0].contains(r#""dry_run":false"#), "{}", reqs[0]);
        assert!(reqs[1].contains(r#""dry_run":true"#), "{}", reqs[1]);
    }

    #[tokio::test]
    async fn cleanup_errors_are_descriptive() {
        for (status, needle) in [
            (403, "superuser"),
            (404, "2.11.0"),
            (409, "already running"),
            (500, "Cleanup failed"),
        ] {
            let server = MockServer::start(vec![MockResponse::json(status, "{}")]);
            let client = RepoClient::new(&server.url, "k", test_client()).unwrap();
            let err = client.cleanup(true).await.unwrap_err().to_string();
            assert!(err.contains(needle), "{status}: {err}");
        }
    }
}
