use anyhow::{Context, Result, bail};
use flate2::read::GzDecoder;
use std::collections::BTreeMap;
use std::io::Read;
use tracing::debug;

use crate::config::DebRepoLayout;
use crate::models::{PackageVersion, RemotePackage};

#[derive(Debug)]
pub struct DebRepoSource {
    pub url: String,
    pub layout: DebRepoLayout,
    pub suites: Vec<String>,
    pub components: Vec<String>,
    pub architectures: Vec<String>,
    pub package_filter: Vec<String>,
    pub filename_filter: Option<glob::Pattern>,
    pub verify_gpg: bool,
    pub gpg_key: Option<String>,
    client: reqwest::Client,
}

impl DebRepoSource {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        url: &str,
        layout: DebRepoLayout,
        suites: Vec<String>,
        components: Vec<String>,
        architectures: Vec<String>,
        package_filter: Vec<String>,
        filename_filter: Option<&str>,
        verify_gpg: bool,
        gpg_key: Option<&str>,
    ) -> Result<Self> {
        if layout == DebRepoLayout::Debian && suites.is_empty() {
            bail!("deb_repo: suites must not be empty");
        }
        if layout == DebRepoLayout::Debian && components.is_empty() {
            bail!("deb_repo: components must not be empty");
        }
        if layout == DebRepoLayout::Debian && architectures.is_empty() {
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
            layout,
            suites,
            components,
            architectures,
            package_filter,
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
        if self.layout == DebRepoLayout::Flat {
            if self.verify_gpg {
                self.verify_flat_gpg()
                    .await
                    .context("GPG verification failed for flat repository")?;
            }

            let packages = self.fetch_flat_packages().await?;
            return Ok(self.limit_packages_per_group(packages, n));
        }

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

        Ok(self.limit_packages_per_group(all, n))
    }

    fn limit_packages_per_group(
        &self,
        packages: Vec<RemotePackage>,
        n: usize,
    ) -> Vec<RemotePackage> {
        let mut grouped: BTreeMap<(String, String), Vec<RemotePackage>> = BTreeMap::new();

        for pkg in packages {
            let key = (
                pkg.package_name.clone().unwrap_or_default(),
                pkg.architecture.clone().unwrap_or_default(),
            );
            grouped.entry(key).or_default().push(pkg);
        }

        let mut limited = Vec::new();
        for (_, mut group) in grouped {
            group.sort_by(|a, b| b.version.cmp(&a.version));
            group.dedup_by(|a, b| a.filename == b.filename);
            group.truncate(n);
            limited.extend(group);
        }

        limited.sort_by(|a, b| {
            a.package_name
                .cmp(&b.package_name)
                .then(a.architecture.cmp(&b.architecture))
                .then(b.version.cmp(&a.version))
        });
        limited
    }

    async fn fetch_flat_packages(&self) -> Result<Vec<RemotePackage>> {
        let base = &self.url;
        let text = if let Ok(t) = self.fetch_index_gz(&format!("{base}/Packages.gz")).await {
            t
        } else {
            self.fetch_index_plain(&format!("{base}/Packages"))
                .await
                .with_context(|| format!("Could not fetch flat Packages index from {base}"))?
        };

        debug!("Fetched flat Packages index ({} bytes)", text.len());
        Ok(self.parse_packages(&text))
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
            let mut architecture = None::<&str>;

            for line in stanza.lines() {
                if let Some(v) = line.strip_prefix("Package: ") {
                    pkg_name = Some(v.trim());
                } else if let Some(v) = line.strip_prefix("Version: ") {
                    version = Some(v.trim());
                } else if let Some(v) = line.strip_prefix("Filename: ") {
                    filename = Some(v.trim());
                } else if let Some(v) = line.strip_prefix("Architecture: ") {
                    architecture = Some(v.trim());
                }
            }

            let (Some(name), Some(ver), Some(file), Some(arch)) =
                (pkg_name, version, filename, architecture)
            else {
                continue;
            };

            if !self.architectures.is_empty()
                && arch != "all"
                && !self
                    .architectures
                    .iter()
                    .any(|configured| configured == arch)
            {
                continue;
            }

            // Apply package_filter (exact name match against any configured name).
            if !self.package_filter.is_empty() && !self.package_filter.iter().any(|f| f == name) {
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
                package_name: Some(name.to_string()),
                architecture: Some(arch.to_string()),
            });
        }

        result
    }

    /// Verify the InRelease (or Release + Release.gpg) signature for a suite.
    /// Requires the `gpg` binary to be available. Skips gracefully if no
    /// gpg_key is configured.
    async fn verify_suite_gpg(&self, suite: &str) -> Result<()> {
        self.verify_gpg_at(
            &format!("{}/dists/{suite}", self.url),
            &format!("suite '{suite}'"),
        )
        .await
    }

    async fn verify_flat_gpg(&self) -> Result<()> {
        self.verify_gpg_at(&self.url, "flat repository").await
    }

    async fn verify_gpg_at(&self, release_base_url: &str, label: &str) -> Result<()> {
        let Some(ref key_source) = self.gpg_key else {
            // verify_gpg is true but no key configured — warn and skip.
            debug!("GPG verification enabled but no gpg_key set for {label}; skipping");
            return Ok(());
        };

        // Check that the gpg binary is available before attempting verification.
        if std::process::Command::new("gpg")
            .arg("--version")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_err()
        {
            bail!(
                "gpg binary not found. Install gpg or set verify_gpg: false \
                 in the project config to skip signature verification"
            );
        }

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
        let inrelease_url = format!("{release_base_url}/InRelease");
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
            debug!("GPG signature verified for {label}");
            Ok(())
        } else {
            bail!(
                "GPG signature verification failed for {label}: {}",
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
            DebRepoLayout::Debian,
            vec!["bookworm".to_string()],
            vec!["main".to_string()],
            vec!["amd64".to_string()],
            vec![],
            None,
            false,
            None,
        )
        .unwrap()
    }

    fn flat_source(url: &str) -> DebRepoSource {
        DebRepoSource::new(
            url,
            DebRepoLayout::Flat,
            vec![],
            vec![],
            vec![],
            vec![],
            None,
            false,
            None,
        )
        .unwrap()
    }

    fn packages_text(entries: &[(&str, &str, &str, &str)]) -> String {
        entries
            .iter()
            .map(|(name, ver, arch, file)| {
                format!("Package: {name}\nVersion: {ver}\nArchitecture: {arch}\nFilename: {file}\n")
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
            "amd64",
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
            (
                "nginx",
                "1.24.0-1",
                "amd64",
                "pool/nginx/nginx_1.24.0-1_amd64.deb",
            ),
            (
                "curl",
                "7.88.1-1",
                "amd64",
                "pool/curl/curl_7.88.1-1_amd64.deb",
            ),
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
        s.package_filter = vec!["nginx".to_string()];
        let text = packages_text(&[
            (
                "nginx",
                "1.24.0-1",
                "amd64",
                "pool/nginx/nginx_1.24.0-1_amd64.deb",
            ),
            (
                "curl",
                "7.88.1-1",
                "amd64",
                "pool/curl/curl_7.88.1-1_amd64.deb",
            ),
        ]);
        let pkgs = s.parse_packages(&text);
        assert_eq!(pkgs.len(), 1);
        assert_eq!(pkgs[0].filename, "nginx_1.24.0-1_amd64.deb");
    }

    #[test]
    fn package_filter_no_match_returns_empty() {
        let mut s = source("https://example.com");
        s.package_filter = vec!["apache2".to_string()];
        let text = packages_text(&[(
            "nginx",
            "1.24.0-1",
            "amd64",
            "pool/nginx/nginx_1.24.0-1_amd64.deb",
        )]);
        assert!(s.parse_packages(&text).is_empty());
    }

    #[test]
    fn package_filter_matches_any_configured_name() {
        let mut s = source("https://example.com");
        s.package_filter = vec!["nginx".to_string(), "curl".to_string()];
        let text = packages_text(&[
            (
                "nginx",
                "1.24.0-1",
                "amd64",
                "pool/nginx/nginx_1.24.0-1_amd64.deb",
            ),
            (
                "curl",
                "7.88.1-1",
                "amd64",
                "pool/curl/curl_7.88.1-1_amd64.deb",
            ),
            (
                "apache2",
                "2.4.0-1",
                "amd64",
                "pool/apache2/apache2_2.4.0-1_amd64.deb",
            ),
        ]);
        let pkgs = s.parse_packages(&text);
        assert_eq!(pkgs.len(), 2);
        assert_eq!(pkgs[0].filename, "nginx_1.24.0-1_amd64.deb");
        assert_eq!(pkgs[1].filename, "curl_7.88.1-1_amd64.deb");
    }

    #[test]
    fn filename_filter_matches_glob() {
        let mut s = source("https://example.com");
        s.filename_filter = Some(glob::Pattern::new("nginx_*.deb").unwrap());
        let text = packages_text(&[
            (
                "nginx",
                "1.24.0-1",
                "amd64",
                "pool/nginx/nginx_1.24.0-1_amd64.deb",
            ),
            (
                "nginx-extras",
                "1.24.0-1",
                "amd64",
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
        let text = packages_text(&[(
            "nginx",
            "1.24.0-1",
            "amd64",
            "pool/nginx/nginx_1.24.0-1_amd64.deb",
        )]);
        assert!(s.parse_packages(&text).is_empty());
    }

    #[test]
    fn download_url_constructed_from_repo_base_and_filename_field() {
        let s = source("https://apt.example.com/debian/");
        let text = packages_text(&[(
            "tool",
            "1.0.0",
            "amd64",
            "pool/main/t/tool/tool_1.0.0_amd64.deb",
        )]);
        let pkgs = s.parse_packages(&text);
        assert_eq!(
            pkgs[0].download_url,
            "https://apt.example.com/debian/pool/main/t/tool/tool_1.0.0_amd64.deb"
        );
    }

    #[test]
    fn flat_obs_download_url_constructed_from_repo_base_and_relative_filename() {
        let s = flat_source(
            "https://download.opensuse.org/repositories/home:/CZ-NIC:/datovka-latest/Debian_13/",
        );
        let text = packages_text(&[(
            "datovka",
            "4.29.4-1",
            "amd64",
            "amd64/datovka_4.29.4-1_amd64.deb",
        )]);
        let pkgs = s.parse_packages(&text);
        assert_eq!(pkgs.len(), 1);
        assert_eq!(pkgs[0].filename, "datovka_4.29.4-1_amd64.deb");
        assert_eq!(
            pkgs[0].download_url,
            "https://download.opensuse.org/repositories/home:/CZ-NIC:/datovka-latest/Debian_13/amd64/datovka_4.29.4-1_amd64.deb"
        );
    }

    #[test]
    fn trailing_slash_on_url_not_doubled() {
        let s = source("https://example.com/repo/");
        let text = packages_text(&[("p", "1.0", "amd64", "pool/p_1.0_amd64.deb")]);
        let pkgs = s.parse_packages(&text);
        assert!(!pkgs[0].download_url.contains("//pool"), "double slash");
    }

    #[test]
    fn invalid_filename_filter_is_rejected() {
        let err = DebRepoSource::new(
            "https://example.com",
            DebRepoLayout::Debian,
            vec!["bookworm".to_string()],
            vec!["main".to_string()],
            vec!["amd64".to_string()],
            vec![],
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
            DebRepoLayout::Debian,
            vec![],
            vec!["main".to_string()],
            vec!["amd64".to_string()],
            vec![],
            None,
            false,
            None,
        )
        .unwrap_err();
        assert!(err.to_string().contains("suites must not be empty"));
    }

    #[test]
    fn limit_packages_per_group_deduplicates_and_takes_top_n() {
        let s = source("https://example.com");
        let pkgs = vec![
            RemotePackage {
                filename: "tool_2.0.0_all.deb".to_string(),
                version: PackageVersion::parse("2.0.0"),
                download_url: "https://example.com/tool_2.0.0_all.deb".to_string(),
                package_name: Some("tool".to_string()),
                architecture: Some("all".to_string()),
            },
            RemotePackage {
                filename: "tool_1.0.0_amd64.deb".to_string(),
                version: PackageVersion::parse("1.0.0"),
                download_url: "https://example.com/tool_1.0.0_amd64.deb".to_string(),
                package_name: Some("tool".to_string()),
                architecture: Some("amd64".to_string()),
            },
            RemotePackage {
                filename: "tool_2.0.0_all.deb".to_string(),
                version: PackageVersion::parse("2.0.0"),
                download_url: "https://example.com/tool_2.0.0_all.deb".to_string(),
                package_name: Some("tool".to_string()),
                architecture: Some("all".to_string()),
            },
        ];
        let pkgs = s.limit_packages_per_group(pkgs, 1);

        assert_eq!(pkgs.len(), 2);
        assert!(pkgs.iter().any(|pkg| pkg.filename == "tool_2.0.0_all.deb"));
        assert!(
            pkgs.iter()
                .any(|pkg| pkg.filename == "tool_1.0.0_amd64.deb")
        );
    }

    #[test]
    fn parse_packages_filters_out_non_requested_architectures_in_flat_layout() {
        let mut s = flat_source("https://example.com");
        s.architectures = vec!["amd64".to_string()];
        let text = packages_text(&[
            (
                "datovka",
                "4.29.4-1",
                "amd64",
                "amd64/datovka_4.29.4-1_amd64.deb",
            ),
            (
                "datovka",
                "4.29.4-1",
                "arm64",
                "arm64/datovka_4.29.4-1_arm64.deb",
            ),
            (
                "datovka",
                "4.29.4-1",
                "armhf",
                "armhf/datovka_4.29.4-1_armhf.deb",
            ),
        ]);

        let pkgs = s.parse_packages(&text);

        assert_eq!(pkgs.len(), 1);
        assert_eq!(pkgs[0].filename, "datovka_4.29.4-1_amd64.deb");
    }

    #[test]
    fn parse_packages_keeps_arch_all_for_requested_architectures() {
        let mut s = flat_source("https://example.com");
        s.architectures = vec!["amd64".to_string()];
        let text = packages_text(&[
            (
                "shared-data",
                "1.2.3-1",
                "all",
                "all/shared-data_1.2.3-1_all.deb",
            ),
            (
                "shared-data",
                "1.2.3-1",
                "arm64",
                "arm64/shared-data_1.2.3-1_arm64.deb",
            ),
        ]);

        let pkgs = s.parse_packages(&text);

        assert_eq!(pkgs.len(), 1);
        assert_eq!(pkgs[0].architecture.as_deref(), Some("all"));
    }

    #[test]
    fn limit_packages_per_group_keeps_multiple_package_filters() {
        let s = source("https://example.com");
        let pkgs = vec![
            RemotePackage {
                filename: "datovka_4.29.4-1_amd64.deb".to_string(),
                version: PackageVersion::parse("4.29.4-1"),
                download_url: "https://example.com/datovka_4.29.4-1_amd64.deb".to_string(),
                package_name: Some("datovka".to_string()),
                architecture: Some("amd64".to_string()),
            },
            RemotePackage {
                filename: "libdatovka8_4.29.4-1_amd64.deb".to_string(),
                version: PackageVersion::parse("4.29.4-1"),
                download_url: "https://example.com/libdatovka8_4.29.4-1_amd64.deb".to_string(),
                package_name: Some("libdatovka8".to_string()),
                architecture: Some("amd64".to_string()),
            },
            RemotePackage {
                filename: "libdatovka0_4.29.4-1_amd64.deb".to_string(),
                version: PackageVersion::parse("4.29.4-1"),
                download_url: "https://example.com/libdatovka0_4.29.4-1_amd64.deb".to_string(),
                package_name: Some("libdatovka0".to_string()),
                architecture: Some("amd64".to_string()),
            },
        ];

        let limited = s.limit_packages_per_group(pkgs, 1);

        assert_eq!(limited.len(), 3);
    }

    // ── fetch_latest over a mock HTTP server ───────────────────────────────

    use crate::test_util::{MockResponse, MockServer};

    #[tokio::test]
    async fn fetch_latest_returns_packages_from_plain_index() {
        // Packages.gz returns 404, Packages succeeds.
        let body = packages_text(&[(
            "nginx",
            "1.24.0-1",
            "amd64",
            "pool/nginx_1.24.0-1_amd64.deb",
        )]);
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
    async fn fetch_latest_flat_layout_reads_root_packages_index() {
        let body = packages_text(&[(
            "datovka",
            "4.29.4-1",
            "amd64",
            "amd64/datovka_4.29.4-1_amd64.deb",
        )]);
        let server = MockServer::start(vec![
            MockResponse::json(404, "not found"),
            MockResponse::json(200, &body),
        ]);
        let s = flat_source("placeholder").with_url(&server.url);

        let pkgs = s.fetch_latest(10).await.unwrap();
        assert_eq!(pkgs.len(), 1);
        assert_eq!(pkgs[0].filename, "datovka_4.29.4-1_amd64.deb");

        let requests = server.requests();
        assert!(requests[0].starts_with("GET /Packages.gz "));
        assert!(requests[1].starts_with("GET /Packages "));
    }

    #[tokio::test]
    async fn fetch_latest_debian_layout_keeps_dists_path() {
        let body = packages_text(&[(
            "nginx",
            "1.24.0-1",
            "amd64",
            "pool/nginx_1.24.0-1_amd64.deb",
        )]);
        let server = MockServer::start(vec![
            MockResponse::json(404, "not found"),
            MockResponse::json(200, &body),
        ]);
        let s = source("placeholder").with_url(&server.url);

        let pkgs = s.fetch_latest(10).await.unwrap();
        assert_eq!(pkgs.len(), 1);

        let requests = server.requests();
        assert!(requests[0].starts_with("GET /dists/bookworm/main/binary-amd64/Packages.gz "));
        assert!(requests[1].starts_with("GET /dists/bookworm/main/binary-amd64/Packages "));
    }

    #[tokio::test]
    async fn fetch_latest_decompresses_gz_index() {
        use flate2::Compression;
        use flate2::write::GzEncoder;
        use std::io::Write;

        let plain = packages_text(&[("curl", "7.88.1-1", "amd64", "pool/curl_7.88.1-1_amd64.deb")]);
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
            (
                "nginx",
                "1.24.0-1",
                "amd64",
                "pool/nginx_1.24.0-1_amd64.deb",
            ),
            ("curl", "7.88.1-1", "amd64", "pool/curl_7.88.1-1_amd64.deb"),
        ]);
        let server = MockServer::start(vec![
            MockResponse::json(404, "not found"),
            MockResponse::json(200, &body),
        ]);
        let mut s = source("placeholder").with_url(&server.url);
        s.package_filter = vec!["nginx".to_string()];

        let pkgs = s.fetch_latest(10).await.unwrap();
        assert_eq!(pkgs.len(), 1);
        assert_eq!(pkgs[0].filename, "nginx_1.24.0-1_amd64.deb");
    }

    #[tokio::test]
    async fn fetch_latest_truncates_to_n() {
        let body = packages_text(&[
            (
                "nginx",
                "1.26.0-1",
                "amd64",
                "pool/nginx_1.26.0-1_amd64.deb",
            ),
            (
                "nginx",
                "1.24.0-1",
                "amd64",
                "pool/nginx_1.24.0-1_amd64.deb",
            ),
            (
                "nginx",
                "1.22.0-1",
                "amd64",
                "pool/nginx_1.22.0-1_amd64.deb",
            ),
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
    async fn fetch_latest_keeps_n_per_package_and_architecture_group() {
        let body = packages_text(&[
            (
                "datovka",
                "4.29.5-1",
                "amd64",
                "pool/datovka_4.29.5-1_amd64.deb",
            ),
            (
                "datovka",
                "4.29.4-1",
                "amd64",
                "pool/datovka_4.29.4-1_amd64.deb",
            ),
            (
                "datovka",
                "4.29.3-1",
                "amd64",
                "pool/datovka_4.29.3-1_amd64.deb",
            ),
            (
                "libdatovka8",
                "4.29.5-1",
                "amd64",
                "pool/libdatovka8_4.29.5-1_amd64.deb",
            ),
            (
                "libdatovka8",
                "4.29.4-1",
                "amd64",
                "pool/libdatovka8_4.29.4-1_amd64.deb",
            ),
            (
                "libdatovka8",
                "4.29.3-1",
                "amd64",
                "pool/libdatovka8_4.29.3-1_amd64.deb",
            ),
            (
                "libdatovka0",
                "4.29.5-1",
                "amd64",
                "pool/libdatovka0_4.29.5-1_amd64.deb",
            ),
            (
                "libdatovka0",
                "4.29.4-1",
                "amd64",
                "pool/libdatovka0_4.29.4-1_amd64.deb",
            ),
            (
                "libdatovka0",
                "4.29.3-1",
                "amd64",
                "pool/libdatovka0_4.29.3-1_amd64.deb",
            ),
        ]);
        let server = MockServer::start(vec![
            MockResponse::json(404, "not found"),
            MockResponse::json(200, &body),
        ]);
        let mut s = flat_source("placeholder").with_url(&server.url);
        s.package_filter = vec![
            "datovka".to_string(),
            "libdatovka8".to_string(),
            "libdatovka0".to_string(),
        ];
        s.architectures = vec!["amd64".to_string()];

        let pkgs = s.fetch_latest(2).await.unwrap();

        assert_eq!(pkgs.len(), 6);
        assert_eq!(
            pkgs.iter()
                .filter(|p| p.package_name.as_deref() == Some("datovka"))
                .count(),
            2
        );
        assert_eq!(
            pkgs.iter()
                .filter(|p| p.package_name.as_deref() == Some("libdatovka8"))
                .count(),
            2
        );
        assert_eq!(
            pkgs.iter()
                .filter(|p| p.package_name.as_deref() == Some("libdatovka0"))
                .count(),
            2
        );
        assert!(!pkgs.iter().any(|p| p.filename.contains("4.29.3-1")));
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

    // ── constructor validation tests ───────────────────────────────────────

    #[test]
    fn empty_components_rejected() {
        let err = DebRepoSource::new(
            "https://example.com",
            DebRepoLayout::Debian,
            vec!["bookworm".to_string()],
            vec![],
            vec!["amd64".to_string()],
            vec![],
            None,
            false,
            None,
        )
        .unwrap_err();
        assert!(err.to_string().contains("components must not be empty"));
    }

    #[test]
    fn empty_architectures_rejected() {
        let err = DebRepoSource::new(
            "https://example.com",
            DebRepoLayout::Debian,
            vec!["bookworm".to_string()],
            vec!["main".to_string()],
            vec![],
            vec![],
            None,
            false,
            None,
        )
        .unwrap_err();
        assert!(err.to_string().contains("architectures must not be empty"));
    }

    // ── GPG verification tests ─────────────────────────────────────────────

    fn source_with_gpg(url: &str, gpg_key: Option<&str>) -> DebRepoSource {
        DebRepoSource::new(
            url,
            DebRepoLayout::Debian,
            vec!["stable".to_string()],
            vec!["main".to_string()],
            vec!["amd64".to_string()],
            vec![],
            None,
            true,
            gpg_key,
        )
        .unwrap()
    }

    fn flat_source_with_gpg(url: &str, gpg_key: Option<&str>) -> DebRepoSource {
        DebRepoSource::new(
            url,
            DebRepoLayout::Flat,
            vec![],
            vec![],
            vec![],
            vec![],
            None,
            true,
            gpg_key,
        )
        .unwrap()
    }

    #[tokio::test]
    async fn verify_gpg_false_skips_verification() {
        // verify_gpg: false should succeed even if the server returns nothing
        // useful (no InRelease endpoint needed).
        let body = packages_text(&[(
            "nginx",
            "1.24.0-1",
            "amd64",
            "pool/nginx_1.24.0-1_amd64.deb",
        )]);
        let server = MockServer::start(vec![
            MockResponse::json(404, "not found"), // Packages.gz
            MockResponse::json(200, &body),       // Packages
        ]);
        let s = source("placeholder").with_url(&server.url);

        // source() helper uses verify_gpg: false
        let pkgs = s.fetch_latest(10).await.unwrap();
        assert_eq!(pkgs.len(), 1);
    }

    #[tokio::test]
    async fn gpg_verify_skips_when_no_key_configured() {
        // verify_gpg: true but gpg_key: None → should skip GPG and proceed
        let body = packages_text(&[(
            "nginx",
            "1.24.0-1",
            "amd64",
            "pool/nginx_1.24.0-1_amd64.deb",
        )]);
        let server = MockServer::start(vec![
            MockResponse::json(404, "not found"), // Packages.gz
            MockResponse::json(200, &body),       // Packages
        ]);
        let s = source_with_gpg("placeholder", None).with_url(&server.url);

        let pkgs = s.fetch_latest(10).await.unwrap();
        assert_eq!(pkgs.len(), 1);
    }

    #[tokio::test]
    async fn gpg_verify_fails_when_inrelease_fetch_fails() {
        // verify_gpg: true, gpg_key set, but InRelease returns 404
        let server = MockServer::start(vec![
            MockResponse::json(404, "not found"), // InRelease fetch
        ]);
        let s = source_with_gpg("placeholder", Some("inline-key-data")).with_url(&server.url);

        let err = s.fetch_latest(10).await.unwrap_err();
        assert!(
            err.to_string().contains("GPG verification failed")
                || err.to_string().contains("InRelease"),
            "unexpected error: {err}"
        );
    }

    // ── GPG tests that require the gpg binary ──────────────────────────────

    use crate::test_util::gpg_available;

    /// Generate a temporary GPG key pair and return (public_key_armor, gnupghome).
    /// The returned tempdir must be kept alive for the key to remain valid.
    fn generate_test_gpg_key() -> (String, tempfile::TempDir) {
        let gnupghome = tempfile::tempdir().unwrap();
        let key_params = "%no-protection\nKey-Type: RSA\nKey-Length: 2048\nName-Real: Test Key\nName-Email: test@example.com\n%commit\n";
        let params_file = gnupghome.path().join("key-params.txt");
        std::fs::write(&params_file, key_params).unwrap();

        let status = std::process::Command::new("gpg")
            .args([
                "--homedir",
                gnupghome.path().to_str().unwrap(),
                "--batch",
                "--gen-key",
                params_file.to_str().unwrap(),
            ])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .unwrap();
        assert!(status.success(), "gpg --gen-key failed");

        let output = std::process::Command::new("gpg")
            .args([
                "--homedir",
                gnupghome.path().to_str().unwrap(),
                "--armor",
                "--export",
                "test@example.com",
            ])
            .output()
            .unwrap();
        assert!(output.status.success(), "gpg --export failed");

        let pubkey = String::from_utf8(output.stdout).unwrap();
        (pubkey, gnupghome)
    }

    /// Create a clearsigned InRelease-like file using the test key.
    fn sign_inrelease(content: &str, gnupghome: &std::path::Path) -> String {
        let tmp = tempfile::tempdir().unwrap();
        let input_file = tmp.path().join("Release");
        std::fs::write(&input_file, content).unwrap();

        let output = std::process::Command::new("gpg")
            .args([
                "--homedir",
                gnupghome.to_str().unwrap(),
                "--batch",
                "--yes",
                "--clearsign",
                "--local-user",
                "test@example.com",
                input_file.to_str().unwrap(),
            ])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "gpg --clearsign failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );

        let signed_file = tmp.path().join("Release.asc");
        std::fs::read_to_string(&signed_file).unwrap()
    }

    #[tokio::test]
    async fn gpg_verify_succeeds_with_valid_signature() {
        if !gpg_available() {
            eprintln!("skipping: gpg not available");
            return;
        }

        let (pubkey, gnupghome) = generate_test_gpg_key();
        let inrelease_content =
            "Suite: stable\nCodename: stable\nDate: Mon, 18 Aug 2026 00:00:00 UTC\n";
        let signed_inrelease = sign_inrelease(inrelease_content, gnupghome.path());

        // Mock server: serves InRelease (for GPG check), then Packages index
        let body = packages_text(&[(
            "nginx",
            "1.24.0-1",
            "amd64",
            "pool/nginx_1.24.0-1_amd64.deb",
        )]);
        let server = MockServer::start(vec![
            MockResponse::json(200, &signed_inrelease), // InRelease
            MockResponse::json(404, "not found"),       // Packages.gz
            MockResponse::json(200, &body),             // Packages
        ]);
        let s = source_with_gpg("placeholder", Some(&pubkey)).with_url(&server.url);

        let pkgs = s.fetch_latest(10).await.unwrap();
        assert_eq!(pkgs.len(), 1);
        assert_eq!(pkgs[0].filename, "nginx_1.24.0-1_amd64.deb");
    }

    #[tokio::test]
    async fn gpg_verify_flat_layout_uses_root_inrelease() {
        if !gpg_available() {
            eprintln!("skipping: gpg not available");
            return;
        }

        let (pubkey, gnupghome) = generate_test_gpg_key();
        let signed_inrelease = sign_inrelease("Codename: Debian_13\n", gnupghome.path());
        let body = packages_text(&[(
            "datovka",
            "4.29.4-1",
            "amd64",
            "amd64/datovka_4.29.4-1_amd64.deb",
        )]);
        let server = MockServer::start(vec![
            MockResponse::json(200, &signed_inrelease),
            MockResponse::json(404, "not found"),
            MockResponse::json(200, &body),
        ]);
        let s = flat_source_with_gpg("placeholder", Some(&pubkey)).with_url(&server.url);

        let pkgs = s.fetch_latest(10).await.unwrap();
        assert_eq!(pkgs.len(), 1);
        assert_eq!(pkgs[0].filename, "datovka_4.29.4-1_amd64.deb");

        let requests = server.requests();
        assert!(requests[0].starts_with("GET /InRelease "));
        assert!(requests[1].starts_with("GET /Packages.gz "));
        assert!(requests[2].starts_with("GET /Packages "));
    }

    #[tokio::test]
    async fn gpg_verify_key_from_url() {
        if !gpg_available() {
            eprintln!("skipping: gpg not available");
            return;
        }

        let (pubkey, gnupghome) = generate_test_gpg_key();
        let inrelease_content = "Suite: stable\nCodename: stable\n";
        let signed_inrelease = sign_inrelease(inrelease_content, gnupghome.path());

        let body = packages_text(&[("curl", "7.88.1-1", "amd64", "pool/curl_7.88.1-1_amd64.deb")]);

        // The key is served via HTTP (first request), then InRelease, then Packages
        let server = MockServer::start(vec![
            MockResponse::json(200, &pubkey),           // GPG key fetch
            MockResponse::json(200, &signed_inrelease), // InRelease
            MockResponse::json(404, "not found"),       // Packages.gz
            MockResponse::json(200, &body),             // Packages
        ]);

        // gpg_key is a URL pointing to the mock server
        let key_url = format!("{}/key.asc", server.url);
        let s = source_with_gpg("placeholder", Some(&key_url)).with_url(&server.url);

        let pkgs = s.fetch_latest(10).await.unwrap();
        assert_eq!(pkgs.len(), 1);
        assert_eq!(pkgs[0].filename, "curl_7.88.1-1_amd64.deb");
    }

    #[tokio::test]
    async fn gpg_verify_key_inline() {
        if !gpg_available() {
            eprintln!("skipping: gpg not available");
            return;
        }

        let (pubkey, gnupghome) = generate_test_gpg_key();
        let inrelease_content = "Suite: stable\nCodename: stable\n";
        let signed_inrelease = sign_inrelease(inrelease_content, gnupghome.path());

        let body = packages_text(&[("tool", "2.0.0", "amd64", "pool/tool_2.0.0_amd64.deb")]);
        let server = MockServer::start(vec![
            MockResponse::json(200, &signed_inrelease), // InRelease
            MockResponse::json(404, "not found"),       // Packages.gz
            MockResponse::json(200, &body),             // Packages
        ]);

        // gpg_key is the inline ASCII-armored key (not a URL)
        let s = source_with_gpg("placeholder", Some(&pubkey)).with_url(&server.url);

        let pkgs = s.fetch_latest(10).await.unwrap();
        assert_eq!(pkgs.len(), 1);
        assert_eq!(pkgs[0].filename, "tool_2.0.0_amd64.deb");
    }

    #[tokio::test]
    async fn gpg_verify_fails_with_wrong_key() {
        if !gpg_available() {
            eprintln!("skipping: gpg not available");
            return;
        }

        // Generate two different key pairs — sign with one, verify with the other
        let (_pubkey1, gnupghome1) = generate_test_gpg_key();
        let (pubkey2, _gnupghome2) = generate_test_gpg_key();

        let inrelease_content = "Suite: stable\nCodename: stable\n";
        let signed_inrelease = sign_inrelease(inrelease_content, gnupghome1.path());

        let server = MockServer::start(vec![
            MockResponse::json(200, &signed_inrelease), // InRelease (signed with key1)
        ]);

        // Use pubkey2 for verification (wrong key)
        let s = source_with_gpg("placeholder", Some(&pubkey2)).with_url(&server.url);

        let err = s.fetch_latest(10).await.unwrap_err();
        assert!(
            err.to_string().contains("GPG")
                || err.to_string().contains("signature")
                || err.to_string().contains("verification failed"),
            "unexpected error: {err}"
        );
    }

    // ── Multi-suite and edge case tests ────────────────────────────────────

    #[tokio::test]
    async fn fetch_latest_multiple_suites_merged() {
        // Source with 2 suites × 1 component × 1 arch = 2 index fetches
        let s = DebRepoSource::new(
            "placeholder",
            DebRepoLayout::Debian,
            vec!["bookworm".to_string(), "bullseye".to_string()],
            vec!["main".to_string()],
            vec!["amd64".to_string()],
            vec![],
            None,
            false,
            None,
        )
        .unwrap();

        let body1 = packages_text(&[(
            "nginx",
            "1.26.0-1",
            "amd64",
            "pool/nginx_1.26.0-1_amd64.deb",
        )]);
        let body2 = packages_text(&[(
            "nginx",
            "1.24.0-1",
            "amd64",
            "pool/nginx_1.24.0-1_amd64.deb",
        )]);

        let server = MockServer::start(vec![
            // bookworm: Packages.gz 404, Packages OK
            MockResponse::json(404, "not found"),
            MockResponse::json(200, &body1),
            // bullseye: Packages.gz 404, Packages OK
            MockResponse::json(404, "not found"),
            MockResponse::json(200, &body2),
        ]);
        let s = s.with_url(&server.url);

        let pkgs = s.fetch_latest(10).await.unwrap();
        assert_eq!(pkgs.len(), 2);
        // Sorted newest first
        assert_eq!(pkgs[0].filename, "nginx_1.26.0-1_amd64.deb");
        assert_eq!(pkgs[1].filename, "nginx_1.24.0-1_amd64.deb");
    }

    #[tokio::test]
    async fn gpg_dearmor_failure_gives_clear_error() {
        if !gpg_available() {
            eprintln!("skipping: gpg not available");
            return;
        }

        // Provide invalid key data (not a valid GPG key) — gpg --dearmor will fail
        let invalid_key = "this is not a valid gpg key at all";

        // Mock: InRelease returns valid content (needed to get to the gpg stage)
        let server = MockServer::start(vec![
            MockResponse::json(200, "Suite: stable\n"), // InRelease
        ]);

        let s = source_with_gpg("placeholder", Some(invalid_key)).with_url(&server.url);

        let err = s.fetch_latest(10).await.unwrap_err();
        assert!(
            err.to_string().contains("GPG")
                || err.to_string().contains("dearmor")
                || err.to_string().contains("verification failed"),
            "unexpected error: {err}"
        );
    }
}
