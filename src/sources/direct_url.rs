use anyhow::{Context, Result};
use std::path::Path;
use tracing::debug;

use crate::models::{PackageVersion, RemotePackage};
use crate::version::{extract_version_from_filename, extract_version_from_package};

pub struct DirectUrlSource {
    pub url: String,
    pub is_latest: bool,
    client: reqwest::Client,
}

impl DirectUrlSource {
    pub fn new(url: &str, is_latest: bool) -> Result<Self> {
        let client = reqwest::Client::builder()
            .user_agent("openrepo-sync/0.1")
            .build()?;
        Ok(Self {
            url: url.to_string(),
            is_latest,
            client,
        })
    }

    pub async fn fetch_latest(&self, _n: usize) -> Result<Vec<RemotePackage>> {
        if self.is_latest {
            self.fetch_latest_url().await
        } else {
            self.fetch_static_url().await
        }
    }

    async fn fetch_static_url(&self) -> Result<Vec<RemotePackage>> {
        let filename = url_filename(&self.url);
        let version = extract_version_from_filename(&filename)
            .unwrap_or(PackageVersion::Raw("0".to_string()));
        Ok(vec![RemotePackage {
            filename,
            version,
            download_url: self.url.clone(),
        }])
    }

    /// For LATEST URLs: download to a temp file, extract version via dpkg/rpm,
    /// then persist the file alongside the temp dir so sync.rs can use it.
    pub async fn fetch_latest_url(&self) -> Result<Vec<RemotePackage>> {
        debug!("Downloading LATEST package from {}", self.url);
        let resp = self
            .client
            .get(&self.url)
            .send()
            .await
            .context("Failed to download LATEST package")?
            .error_for_status()
            .context("Download request error")?;

        // Resolve the real filename from Content-Disposition, final URL, or original URL.
        let original_filename = resolve_filename(&resp, &self.url);
        let ext = Path::new(&original_filename)
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("bin")
            .to_string();

        // Write to a tempfile for version detection
        let tmp = tempfile::Builder::new()
            .suffix(&format!(".{}", ext))
            .tempfile()
            .context("Failed to create temp file")?;
        let tmp_path = tmp.path().to_path_buf();

        let bytes = resp.bytes().await.context("Failed to read download body")?;
        tokio::fs::write(&tmp_path, &bytes)
            .await
            .context("Failed to write temp file")?;

        let version = extract_version_from_package(&tmp_path)
            .with_context(|| format!("Version extraction failed for {}", original_filename))?;

        let versioned_filename = rename_with_version(&original_filename, &version);

        // Persist the temp file to a stable path in the same temp directory.
        // This prevents RAII deletion while keeping the disk clean after sync.
        let stable_path = std::env::temp_dir()
            .join("openrepo-sync-latest")
            .join(&versioned_filename);
        if let Some(parent) = stable_path.parent() {
            std::fs::create_dir_all(parent).context("Failed to create staging directory")?;
        }
        // Persist moves the NamedTempFile to the stable path, preventing deletion.
        tmp.persist(&stable_path)
            .with_context(|| format!("Failed to persist temp file to {}", stable_path.display()))?;

        debug!("Persisted LATEST package to {}", stable_path.display());

        Ok(vec![RemotePackage {
            filename: versioned_filename,
            version,
            download_url: format!("file://{}", stable_path.display()),
        }])
    }
}

pub(crate) fn url_filename(url: &str) -> String {
    url.split('/')
        .next_back()
        .and_then(|s| s.split('?').next())
        .unwrap_or("package")
        .to_string()
}

/// Resolve the real filename from a completed response:
/// 1. Content-Disposition: attachment; filename="foo.deb"
/// 2. Final URL after redirects
/// 3. Original request URL
fn resolve_filename(resp: &reqwest::Response, original_url: &str) -> String {
    // Try Content-Disposition header first
    if let Some(Ok(cd_str)) = resp
        .headers()
        .get("content-disposition")
        .map(|cd| cd.to_str())
    {
        // Match filename="foo.deb" or filename=foo.deb
        let re = regex::Regex::new(r#"filename\*?=(?:"([^"]+)"|([^\s;]+))"#).unwrap();
        if let Some(name) = re.captures(cd_str).and_then(|caps| {
            caps.get(1)
                .or_else(|| caps.get(2))
                .map(|m| m.as_str().trim().to_string())
                .filter(|n| !n.is_empty())
        }) {
            return name;
        }
    }

    // Try final URL after redirects
    let final_url = resp.url().as_str();
    let from_final = url_filename(final_url);
    if !from_final.is_empty() && Path::new(&from_final).extension().is_some() {
        return from_final;
    }

    // Fall back to original URL
    url_filename(original_url)
}

pub(crate) fn rename_with_version(filename: &str, version: &PackageVersion) -> String {
    let version_str = version.to_string();
    if filename.to_uppercase().contains("LATEST") {
        filename
            .replace("LATEST", &version_str)
            .replace("latest", &version_str)
    } else {
        let path = Path::new(filename);
        let stem = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or(filename);
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| format!(".{}", e))
            .unwrap_or_default();
        format!("{}_{}{}", stem, version_str, ext)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::PackageVersion;

    // ── url_filename ───────────────────────────────────────────────────────

    #[test]
    fn url_filename_simple() {
        assert_eq!(
            url_filename("https://example.com/pkg-1.2.3.deb"),
            "pkg-1.2.3.deb"
        );
    }

    #[test]
    fn url_filename_strips_query_string() {
        assert_eq!(
            url_filename("https://example.com/pkg-1.2.3.deb?foo=bar&baz=1"),
            "pkg-1.2.3.deb"
        );
    }

    #[test]
    fn url_filename_empty_path_fallback() {
        assert_eq!(url_filename("https://example.com/"), "");
        // last segment is empty; split('/').last() == Some("")
    }

    // ── rename_with_version ────────────────────────────────────────────────

    #[test]
    fn rename_replaces_latest_uppercase() {
        let ver = PackageVersion::parse("2.1.0");
        assert_eq!(
            rename_with_version("mypkg-LATEST.deb", &ver),
            "mypkg-2.1.0.deb"
        );
    }

    #[test]
    fn rename_replaces_latest_lowercase() {
        let ver = PackageVersion::parse("2.1.0");
        assert_eq!(
            rename_with_version("mypkg-latest.deb", &ver),
            "mypkg-2.1.0.deb"
        );
    }

    #[test]
    fn rename_inserts_version_when_no_latest_keyword() {
        let ver = PackageVersion::parse("3.0.0");
        assert_eq!(rename_with_version("mypkg.deb", &ver), "mypkg_3.0.0.deb");
    }

    #[test]
    fn rename_no_extension() {
        let ver = PackageVersion::parse("1.0.0");
        assert_eq!(rename_with_version("mypkg", &ver), "mypkg_1.0.0");
    }

    #[test]
    fn rename_raw_version() {
        let ver = PackageVersion::Raw("nightly".to_string());
        assert_eq!(
            rename_with_version("tool-LATEST.rpm", &ver),
            "tool-nightly.rpm"
        );
    }

    // ── fetch_static_url: version parsed from URL filename ─────────────────

    #[tokio::test]
    async fn static_url_parses_version_from_filename() {
        let source =
            DirectUrlSource::new("https://example.com/curl-8.5.0_amd64.deb", false).unwrap();
        let pkgs = source.fetch_latest(1).await.unwrap();
        assert_eq!(pkgs.len(), 1);
        assert_eq!(pkgs[0].version, PackageVersion::parse("8.5.0"));
        assert_eq!(pkgs[0].filename, "curl-8.5.0_amd64.deb");
    }

    #[tokio::test]
    async fn static_url_falls_back_to_raw_zero_when_no_version() {
        let source = DirectUrlSource::new("https://example.com/noversion.deb", false).unwrap();
        let pkgs = source.fetch_latest(1).await.unwrap();
        assert_eq!(pkgs[0].version, PackageVersion::Raw("0".to_string()));
    }

    // ── fetch_latest_url: full download + version detection round trip ─────

    use crate::test_util::{MockResponse, MockServer, build_minimal_deb, dpkg_deb_available};

    #[tokio::test]
    async fn latest_url_detects_version_and_persists_package() {
        if !dpkg_deb_available() {
            eprintln!("skipping: dpkg-deb not available");
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let deb = build_minimal_deb(dir.path(), "2.5.0");
        let deb_bytes = std::fs::read(&deb).unwrap();

        let server = MockServer::start(vec![MockResponse::bytes(
            200,
            deb_bytes,
            &[(
                "Content-Disposition",
                r#"attachment; filename="testpkg-LATEST.deb""#,
            )],
        )]);

        let source = DirectUrlSource::new(&format!("{}/download", server.url), true).unwrap();
        let pkgs = source.fetch_latest(1).await.unwrap();

        assert_eq!(pkgs.len(), 1);
        assert_eq!(pkgs[0].version, PackageVersion::parse("2.5.0"));
        // LATEST in the Content-Disposition filename is replaced by the
        // version read out of the package itself.
        assert_eq!(pkgs[0].filename, "testpkg-2.5.0.deb");

        // The package is persisted for sync.rs to pick up via file://.
        let path = pkgs[0].download_url.strip_prefix("file://").unwrap();
        assert!(std::path::Path::new(path).exists());
        let _ = std::fs::remove_file(path);
    }

    #[tokio::test]
    async fn latest_url_names_file_from_final_url_without_content_disposition() {
        if !dpkg_deb_available() {
            eprintln!("skipping: dpkg-deb not available");
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let deb = build_minimal_deb(dir.path(), "3.1.0");
        let deb_bytes = std::fs::read(&deb).unwrap();

        let server = MockServer::start(vec![MockResponse::bytes(200, deb_bytes, &[])]);

        // No Content-Disposition: the filename comes from the URL, which has
        // an extension, and gets the detected version appended.
        let source = DirectUrlSource::new(&format!("{}/mytool.deb", server.url), true).unwrap();
        let pkgs = source.fetch_latest(1).await.unwrap();

        assert_eq!(pkgs[0].version, PackageVersion::parse("3.1.0"));
        assert_eq!(pkgs[0].filename, "mytool_3.1.0.deb");

        let path = pkgs[0].download_url.strip_prefix("file://").unwrap();
        assert!(std::path::Path::new(path).exists());
        let _ = std::fs::remove_file(path);
    }

    #[tokio::test]
    async fn latest_url_download_error_fails() {
        let server = MockServer::start(vec![MockResponse::json(500, "boom")]);
        let source = DirectUrlSource::new(&format!("{}/download", server.url), true).unwrap();
        let err = source.fetch_latest(1).await.unwrap_err();
        assert!(err.to_string().contains("Download request error"));
    }

    #[tokio::test]
    async fn latest_url_unsupported_payload_fails_version_extraction() {
        // No Content-Disposition and no usable extension anywhere: the body
        // is staged as .bin, which version extraction rejects.
        let server = MockServer::start(vec![MockResponse::bytes(200, b"junk".to_vec(), &[])]);
        let source = DirectUrlSource::new(&format!("{}/download", server.url), true).unwrap();
        let err = source.fetch_latest(1).await.unwrap_err();
        assert!(format!("{:#}", err).contains("Version extraction failed"));
    }
}
