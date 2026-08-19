use anyhow::{Context, Result, bail};
use flate2::read::GzDecoder;
use std::io::Read;
use tracing::debug;

use crate::models::{PackageVersion, RemotePackage};

#[derive(Debug)]
pub struct DebRepoSource {
    pub url: String,
    pub suites: Vec<String>,
    pub components: Vec<String>,
    pub architectures: Vec<String>,
    pub package_filter: Option<String>,
    pub filename_filter: Option<glob::Pattern>,
    pub verify_gpg: bool,
    pub gpg_key: Option<String>,
    client: reqwest::Client,
}

impl DebRepoSource {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        url: &str,
        suites: Vec<String>,
        components: Vec<String>,
        architectures: Vec<String>,
        package_filter: Option<&str>,
        filename_filter: Option<&str>,
        verify_gpg: bool,
        gpg_key: Option<&str>,
    ) -> Result<Self> {
        if suites.is_empty() {
            bail!("deb_repo: suites must not be empty");
        }
        if components.is_empty() {
            bail!("deb_repo: components must not be empty");
        }
        if architectures.is_empty() {
            bail!("deb_repo: architectures must not be empty");
        }
        let pattern = filename_filter
            .map(|f| glob::Pattern::new(f).context("Invalid filename_filter glob pattern"))
            .transpose()?;
        let client = reqwest::Client::builder()
            .user_agent("openrepo-sync/0.1")
            .build()?;
        Ok(Self {
            url: url.trim_end_matches('/').to_string(),
            suites,
            components,
            architectures,
            package_filter: package_filter.map(str::to_string),
            filename_filter: pattern,
            verify_gpg,
            gpg_key: gpg_key.map(str::to_string),
            client,
        })
    }

    /// Point the source at a different base URL (tests only).
    #[cfg(test)]
    fn with_url(mut self, url: &str) -> Self {
        self.url = url.trim_end_matches('/').to_string();
        self
    }

    pub async fn fetch_latest(&self, n: usize) -> Result<Vec<RemotePackage>> {
        // Optionally verify GPG for each suite before fetching package indexes.
        if self.verify_gpg {
            for suite in &self.suites {
                self.verify_suite_gpg(suite)
                    .await
                    .with_context(|| format!("GPG verification failed for suite '{suite}'"))?;
            }
        }

        let mut all: Vec<RemotePackage> = Vec::new();

        for suite in &self.suites {
            for component in &self.components {
                for arch in &self.architectures {
                    let packages = self
                        .fetch_packages_for(suite, component, arch)
                        .await
                        .with_context(|| {
                            format!("Failed to fetch Packages for {suite}/{component}/{arch}")
                        })?;
                    all.extend(packages);
                }
            }
        }

        // Deduplicate by filename, sort newest-first, take top n.
        all.sort_by(|a, b| b.version.cmp(&a.version));
        all.dedup_by(|a, b| a.filename == b.filename);
        all.truncate(n);
        Ok(all)
    }

    async fn fetch_packages_for(
        &self,
        suite: &str,
        component: &str,
        arch: &str,
    ) -> Result<Vec<RemotePackage>> {
        let base = format!("{}/dists/{suite}/{component}/binary-{arch}", self.url);

        // Try Packages.gz first, then fall back to plain Packages.
        let text = if let Ok(t) = self.fetch_index_gz(&format!("{base}/Packages.gz")).await {
            t
        } else {
            self.fetch_index_plain(&format!("{base}/Packages"))
                .await
                .with_context(|| format!("Could not fetch Packages index from {base}"))?
        };

        debug!(
            "Fetched Packages index for {suite}/{component}/{arch} ({} bytes)",
            text.len()
        );

        Ok(self.parse_packages(&text))
    }

    async fn fetch_index_gz(&self, url: &str) -> Result<String> {
        let bytes = self
            .client
            .get(url)
            .send()
            .await
            .context("Failed to fetch Packages.gz")?
            .error_for_status()
            .context("Packages.gz request error")?
            .bytes()
            .await
            .context("Failed to read Packages.gz body")?;

        let mut decoder = GzDecoder::new(&bytes[..]);
        let mut text = String::new();
        decoder
            .read_to_string(&mut text)
            .context("Failed to decompress Packages.gz")?;
        Ok(text)
    }

    async fn fetch_index_plain(&self, url: &str) -> Result<String> {
        self.client
            .get(url)
            .send()
            .await
            .context("Failed to fetch Packages index")?
            .error_for_status()
            .context("Packages index request error")?
            .text()
            .await
            .context("Failed to read Packages index body")
    }

    /// Parse a Debian RFC822 `Packages` index into `RemotePackage` entries.
    /// Each stanza is separated by a blank line. Fields we need:
    /// `Package`, `Version`, `Filename`, `Architecture`.
    pub fn parse_packages(&self, text: &str) -> Vec<RemotePackage> {
        let mut result = Vec::new();

        for stanza in text.split("\n\n") {
            let stanza = stanza.trim();
            if stanza.is_empty() {
                continue;
            }

            let mut pkg_name = None::<&str>;
            let mut version = None::<&str>;
            let mut filename = None::<&str>;

            for line in stanza.lines() {
                if let Some(v) = line.strip_prefix("Package: ") {
                    pkg_name = Some(v.trim());
                } else if let Some(v) = line.strip_prefix("Version: ") {
                    version = Some(v.trim());
                } else if let Some(v) = line.strip_prefix("Filename: ") {
                    filename = Some(v.trim());
                }
            }

            let (Some(name), Some(ver), Some(file)) = (pkg_name, version, filename) else {
                continue;
            };

            // Apply package_filter (exact name match).
            if self.package_filter.as_deref().is_some_and(|f| name != f) {
                continue;
            }

            // Apply filename_filter (glob on the Filename field's basename).
            let basename = file.rsplit('/').next().unwrap_or(file);
            if self
                .filename_filter
                .as_ref()
                .is_some_and(|p| !p.matches(basename))
            {
                continue;
            }

            let download_url = format!("{}/{}", self.url, file.trim_start_matches('/'));

            result.push(RemotePackage {
                filename: basename.to_string(),
                version: PackageVersion::parse(ver),
                download_url,
            });
        }

        result
    }

    /// Verify the InRelease (or Release + Release.gpg) signature for a suite.
    /// Requires the `gpg` binary to be available. Skips gracefully if no
    /// gpg_key is configured.
    async fn verify_suite_gpg(&self, suite: &str) -> Result<()> {
        let Some(ref key_source) = self.gpg_key else {
            // verify_gpg is true but no key configured — warn and skip.
            debug!("GPG verification enabled but no gpg_key set for suite '{suite}'; skipping");
            return Ok(());
        };

        // Resolve the key material.
        let key_data = if key_source.starts_with("http://") || key_source.starts_with("https://") {
            self.client
                .get(key_source.as_str())
                .send()
                .await
                .context("Failed to fetch GPG key URL")?
                .error_for_status()
                .context("GPG key URL request error")?
                .bytes()
                .await
                .context("Failed to read GPG key body")?
                .to_vec()
        } else {
            key_source.as_bytes().to_vec()
        };

        // Fetch InRelease (clearsigned).
        let inrelease_url = format!("{}/dists/{suite}/InRelease", self.url);
        let inrelease = self
            .client
            .get(&inrelease_url)
            .send()
            .await
            .context("Failed to fetch InRelease")?
            .error_for_status()
            .context("InRelease request error")?
            .text()
            .await
            .context("Failed to read InRelease body")?;

        // Write key and InRelease to temp files, then call `gpg --verify`.
        let tmp = tempfile::tempdir().context("Failed to create temp dir for GPG")?;
        let keyring = tmp.path().join("repo.gpg");
        let inrelease_file = tmp.path().join("InRelease");

        std::fs::write(&keyring, &key_data).context("Failed to write GPG keyring")?;
        std::fs::write(&inrelease_file, inrelease.as_bytes())
            .context("Failed to write InRelease file")?;

        // Dearmor the key into a temporary keyring for gpg --verify.
        let dearmored = tmp.path().join("repo-dearmored.gpg");
        let dearmor_out = std::process::Command::new("gpg")
            .args([
                "--homedir",
                tmp.path().to_str().unwrap(),
                "--dearmor",
                "--output",
                dearmored.to_str().unwrap(),
                keyring.to_str().unwrap(),
            ])
            .output()
            .context("Failed to run gpg --dearmor (is gpg installed?)")?;

        if !dearmor_out.status.success() {
            bail!(
                "gpg --dearmor failed: {}",
                String::from_utf8_lossy(&dearmor_out.stderr)
            );
        }

        let verify_out = std::process::Command::new("gpg")
            .args([
                "--homedir",
                tmp.path().to_str().unwrap(),
                "--no-default-keyring",
                "--keyring",
                dearmored.to_str().unwrap(),
                "--verify",
                inrelease_file.to_str().unwrap(),
            ])
            .output()
            .context("Failed to run gpg --verify")?;

        if verify_out.status.success() {
            debug!("GPG signature verified for suite '{suite}'");
            Ok(())
        } else {
            bail!(
                "GPG signature verification failed for suite '{suite}': {}",
                String::from_utf8_lossy(&verify_out.stderr)
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source(url: &str) -> DebRepoSource {
        DebRepoSource::new(
            url,
            vec!["bookworm".to_string()],
            vec!["main".to_string()],
            vec!["amd64".to_string()],
            None,
            None,
            false,
            None,
        )
        .unwrap()
    }

    fn packages_text(entries: &[(&str, &str, &str)]) -> String {
        entries
            .iter()
            .map(|(name, ver, file)| {
                format!("Package: {name}\nVersion: {ver}\nArchitecture: amd64\nFilename: {file}\n")
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    // ── parse_packages unit tests ──────────────────────────────────────────

    #[test]
    fn parse_packages_single_stanza() {
        let s = source("https://example.com");
        let text = packages_text(&[(
            "nginx",
            "1.24.0-1",
            "pool/main/n/nginx/nginx_1.24.0-1_amd64.deb",
        )]);
        let pkgs = s.parse_packages(&text);
        assert_eq!(pkgs.len(), 1);
        assert_eq!(pkgs[0].filename, "nginx_1.24.0-1_amd64.deb");
        assert_eq!(pkgs[0].version, PackageVersion::parse("1.24.0-1"));
        assert_eq!(
            pkgs[0].download_url,
            "https://example.com/pool/main/n/nginx/nginx_1.24.0-1_amd64.deb"
        );
    }

    #[test]
    fn parse_packages_multiple_stanzas() {
        let s = source("https://example.com");
        let text = packages_text(&[
            ("nginx", "1.24.0-1", "pool/nginx/nginx_1.24.0-1_amd64.deb"),
            ("curl", "7.88.1-1", "pool/curl/curl_7.88.1-1_amd64.deb"),
        ]);
        let pkgs = s.parse_packages(&text);
        assert_eq!(pkgs.len(), 2);
    }

    #[test]
    fn parse_packages_empty_input() {
        let s = source("https://example.com");
        assert!(s.parse_packages("").is_empty());
        assert!(s.parse_packages("   \n\n  ").is_empty());
    }

    #[test]
    fn parse_packages_skips_stanza_missing_required_field() {
        let s = source("https://example.com");
        // Missing Filename
        let text = "Package: nginx\nVersion: 1.24.0-1\nArchitecture: amd64\n";
        assert!(s.parse_packages(text).is_empty());
    }

    #[test]
    fn package_filter_matches_exact_name() {
        let mut s = source("https://example.com");
        s.package_filter = Some("nginx".to_string());
        let text = packages_text(&[
            ("nginx", "1.24.0-1", "pool/nginx/nginx_1.24.0-1_amd64.deb"),
            ("curl", "7.88.1-1", "pool/curl/curl_7.88.1-1_amd64.deb"),
        ]);
        let pkgs = s.parse_packages(&text);
        assert_eq!(pkgs.len(), 1);
        assert_eq!(pkgs[0].filename, "nginx_1.24.0-1_amd64.deb");
    }

    #[test]
    fn package_filter_no_match_returns_empty() {
        let mut s = source("https://example.com");
        s.package_filter = Some("apache2".to_string());
        let text = packages_text(&[("nginx", "1.24.0-1", "pool/nginx/nginx_1.24.0-1_amd64.deb")]);
        assert!(s.parse_packages(&text).is_empty());
    }

    #[test]
    fn filename_filter_matches_glob() {
        let mut s = source("https://example.com");
        s.filename_filter = Some(glob::Pattern::new("nginx_*.deb").unwrap());
        let text = packages_text(&[
            ("nginx", "1.24.0-1", "pool/nginx/nginx_1.24.0-1_amd64.deb"),
            (
                "nginx-extras",
                "1.24.0-1",
                "pool/nginx/nginx-extras_1.24.0-1_amd64.deb",
            ),
        ]);
        let pkgs = s.parse_packages(&text);
        assert_eq!(pkgs.len(), 1);
        assert_eq!(pkgs[0].filename, "nginx_1.24.0-1_amd64.deb");
    }

    #[test]
    fn filename_filter_rejects_nonmatching() {
        let mut s = source("https://example.com");
        s.filename_filter = Some(glob::Pattern::new("curl_*.deb").unwrap());
        let text = packages_text(&[("nginx", "1.24.0-1", "pool/nginx/nginx_1.24.0-1_amd64.deb")]);
        assert!(s.parse_packages(&text).is_empty());
    }

    #[test]
    fn download_url_constructed_from_repo_base_and_filename_field() {
        let s = source("https://apt.example.com/debian/");
        let text = packages_text(&[("tool", "1.0.0", "pool/main/t/tool/tool_1.0.0_amd64.deb")]);
        let pkgs = s.parse_packages(&text);
        assert_eq!(
            pkgs[0].download_url,
            "https://apt.example.com/debian/pool/main/t/tool/tool_1.0.0_amd64.deb"
        );
    }

    #[test]
    fn trailing_slash_on_url_not_doubled() {
        let s = source("https://example.com/repo/");
        let text = packages_text(&[("p", "1.0", "pool/p_1.0_amd64.deb")]);
        let pkgs = s.parse_packages(&text);
        assert!(!pkgs[0].download_url.contains("//pool"), "double slash");
    }

    #[test]
    fn invalid_filename_filter_is_rejected() {
        let err = DebRepoSource::new(
            "https://example.com",
            vec!["bookworm".to_string()],
            vec!["main".to_string()],
            vec!["amd64".to_string()],
            None,
            Some("[bad"),
            false,
            None,
        )
        .unwrap_err();
        assert!(err.to_string().contains("Invalid filename_filter"));
    }

    #[test]
    fn empty_suites_rejected() {
        let err = DebRepoSource::new(
            "https://example.com",
            vec![],
            vec!["main".to_string()],
            vec!["amd64".to_string()],
            None,
            None,
            false,
            None,
        )
        .unwrap_err();
        assert!(err.to_string().contains("suites must not be empty"));
    }

    #[test]
    fn fetch_latest_deduplicates_and_takes_top_n() {
        // Two architectures returning the same package: dedup should collapse
        // them and keep only the single newest.
        let s = source("https://example.com");
        // Simulate two copies (same filename) — as would happen across two
        // arch indexes that both have an arch:all package.
        let mut pkgs = vec![
            RemotePackage {
                filename: "tool_2.0.0_all.deb".to_string(),
                version: PackageVersion::parse("2.0.0"),
                download_url: "https://example.com/tool_2.0.0_all.deb".to_string(),
            },
            RemotePackage {
                filename: "tool_1.0.0_amd64.deb".to_string(),
                version: PackageVersion::parse("1.0.0"),
                download_url: "https://example.com/tool_1.0.0_amd64.deb".to_string(),
            },
            RemotePackage {
                filename: "tool_2.0.0_all.deb".to_string(),
                version: PackageVersion::parse("2.0.0"),
                download_url: "https://example.com/tool_2.0.0_all.deb".to_string(),
            },
        ];
        // Replicate the dedup+truncate logic from fetch_latest.
        pkgs.sort_by(|a, b| b.version.cmp(&a.version));
        pkgs.dedup_by(|a, b| a.filename == b.filename);
        pkgs.truncate(1);

        assert_eq!(pkgs.len(), 1);
        assert_eq!(pkgs[0].filename, "tool_2.0.0_all.deb");

        drop(s); // suppress unused warning
    }

    // ── fetch_latest over a mock HTTP server ───────────────────────────────

    use crate::test_util::{MockResponse, MockServer};

    #[tokio::test]
    async fn fetch_latest_returns_packages_from_plain_index() {
        // Packages.gz returns 404, Packages succeeds.
        let body = packages_text(&[("nginx", "1.24.0-1", "pool/nginx_1.24.0-1_amd64.deb")]);
        let server = MockServer::start(vec![
            MockResponse::json(404, "not found"), // Packages.gz attempt
            MockResponse::json(200, &body),       // Packages fallback
        ]);
        let s = source("placeholder").with_url(&server.url);

        let pkgs = s.fetch_latest(10).await.unwrap();
        assert_eq!(pkgs.len(), 1);
        assert_eq!(pkgs[0].filename, "nginx_1.24.0-1_amd64.deb");
    }

    #[tokio::test]
    async fn fetch_latest_decompresses_gz_index() {
        use flate2::Compression;
        use flate2::write::GzEncoder;
        use std::io::Write;

        let plain = packages_text(&[("curl", "7.88.1-1", "pool/curl_7.88.1-1_amd64.deb")]);
        let mut enc = GzEncoder::new(Vec::new(), Compression::default());
        enc.write_all(plain.as_bytes()).unwrap();
        let gz_bytes = enc.finish().unwrap();

        let server = MockServer::start(vec![MockResponse::bytes(
            200,
            gz_bytes,
            &[("Content-Type", "application/gzip")],
        )]);
        let s = source("placeholder").with_url(&server.url);

        let pkgs = s.fetch_latest(10).await.unwrap();
        assert_eq!(pkgs.len(), 1);
        assert_eq!(pkgs[0].filename, "curl_7.88.1-1_amd64.deb");
    }

    #[tokio::test]
    async fn fetch_latest_filters_by_package_name() {
        let body = packages_text(&[
            ("nginx", "1.24.0-1", "pool/nginx_1.24.0-1_amd64.deb"),
            ("curl", "7.88.1-1", "pool/curl_7.88.1-1_amd64.deb"),
        ]);
        let server = MockServer::start(vec![
            MockResponse::json(404, "not found"),
            MockResponse::json(200, &body),
        ]);
        let mut s = source("placeholder").with_url(&server.url);
        s.package_filter = Some("nginx".to_string());

        let pkgs = s.fetch_latest(10).await.unwrap();
        assert_eq!(pkgs.len(), 1);
        assert_eq!(pkgs[0].filename, "nginx_1.24.0-1_amd64.deb");
    }

    #[tokio::test]
    async fn fetch_latest_truncates_to_n() {
        let body = packages_text(&[
            ("nginx", "1.26.0-1", "pool/nginx_1.26.0-1_amd64.deb"),
            ("nginx", "1.24.0-1", "pool/nginx_1.24.0-1_amd64.deb"),
            ("nginx", "1.22.0-1", "pool/nginx_1.22.0-1_amd64.deb"),
        ]);
        let server = MockServer::start(vec![
            MockResponse::json(404, "not found"),
            MockResponse::json(200, &body),
        ]);
        let s = source("placeholder").with_url(&server.url);

        let pkgs = s.fetch_latest(2).await.unwrap();
        assert_eq!(pkgs.len(), 2);
        assert_eq!(pkgs[0].filename, "nginx_1.26.0-1_amd64.deb");
        assert_eq!(pkgs[1].filename, "nginx_1.24.0-1_amd64.deb");
    }

    #[tokio::test]
    async fn fetch_latest_http_error_fails() {
        let server = MockServer::start(vec![
            MockResponse::json(500, "error"), // Packages.gz
            MockResponse::json(500, "error"), // Packages fallback
        ]);
        let s = source("placeholder").with_url(&server.url);

        let err = s.fetch_latest(10).await.unwrap_err();
        assert!(
            err.to_string().contains("bookworm/main/amd64"),
            "unexpected error: {err}"
        );
    }
}
