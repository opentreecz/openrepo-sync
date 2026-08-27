use anyhow::{Context, Result, bail};
use quick_xml::Reader;
use quick_xml::events::Event;
use serde::Deserialize;
use std::io::Read;
use tracing::debug;

use crate::models::{PackageVersion, RemotePackage};

/// Source that mirrors packages from an RPM (YUM/DNF) repository by parsing
/// `repodata/repomd.xml` to locate the primary metadata, then fetching and
/// parsing the package list from either `primary.xml` or `primary.sqlite`.
#[derive(Debug)]
pub struct RpmRepoSource {
    pub url: String,
    pub package_filter: Vec<String>,
    pub filename_filter: Option<glob::Pattern>,
    pub verify_gpg: bool,
    pub gpg_key: Option<String>,
    pub architectures: Vec<String>,
    client: reqwest::Client,
}

/// A single package entry parsed from primary metadata.
#[derive(Debug, Clone)]
struct RpmPackageEntry {
    name: String,
    arch: String,
    epoch: u32,
    version: String,
    release: String,
    location_href: String,
}

impl RpmPackageEntry {
    fn to_remote_package(&self, base_url: &str) -> RemotePackage {
        let version_str = if self.epoch > 0 {
            format!("{}:{}-{}", self.epoch, self.version, self.release)
        } else {
            format!("{}-{}", self.version, self.release)
        };
        let basename = self
            .location_href
            .rsplit('/')
            .next()
            .unwrap_or(&self.location_href);
        let download_url = format!(
            "{}/{}",
            base_url.trim_end_matches('/'),
            self.location_href.trim_start_matches('/')
        );

        RemotePackage {
            filename: basename.to_string(),
            version: PackageVersion::parse(&version_str),
            download_url,
            package_name: Some(self.name.clone()),
            architecture: Some(self.arch.clone()),
        }
    }
}

// ── repomd.xml serde structures ────────────────────────────────────────────

#[derive(Debug, Deserialize)]
#[serde(rename = "repomd")]
struct Repomd {
    #[serde(rename = "data", default)]
    entries: Vec<RepomdData>,
}

#[derive(Debug, Deserialize)]
struct RepomdData {
    #[serde(rename = "@type")]
    data_type: String,
    location: RepomdLocation,
}

#[derive(Debug, Deserialize)]
struct RepomdLocation {
    #[serde(rename = "@href")]
    href: String,
}

// ── Implementation ─────────────────────────────────────────────────────────

impl RpmRepoSource {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        url: &str,
        package_filter: Vec<String>,
        filename_filter: Option<&str>,
        verify_gpg: bool,
        gpg_key: Option<&str>,
        architectures: Vec<String>,
    ) -> Result<Self> {
        let pattern = filename_filter
            .map(|f| glob::Pattern::new(f).context("Invalid filename_filter glob pattern"))
            .transpose()?;
        let client = reqwest::Client::builder()
            .user_agent("openrepo-sync/0.1")
            .build()?;
        Ok(Self {
            url: url.trim_end_matches('/').to_string(),
            package_filter,
            filename_filter: pattern,
            verify_gpg,
            gpg_key: gpg_key.map(str::to_string),
            architectures,
            client,
        })
    }

    /// Point the source at a different base URL (tests only).
    #[cfg(test)]
    fn with_url(mut self, url: &str) -> Self {
        self.url = url.trim_end_matches('/').to_string();
        self
    }

    /// Fetch the newest `n` packages from the RPM repository.
    pub async fn fetch_latest(&self, n: usize) -> Result<Vec<RemotePackage>> {
        // Optionally verify GPG signature of repomd.xml
        if self.verify_gpg {
            self.verify_repomd_gpg()
                .await
                .context("GPG verification failed for repomd.xml")?;
        }

        // Fetch and parse repomd.xml
        let repomd = self.fetch_repomd().await?;
        let primary_href = self.find_primary_href(&repomd)?;

        debug!("Primary metadata: {primary_href}");

        // Fetch and decompress the primary metadata
        let raw_bytes = self.fetch_metadata_bytes(&primary_href).await?;
        let decompressed = Self::decompress(&raw_bytes, &primary_href)?;

        // Parse packages from the appropriate format
        let mut packages = if primary_href.contains(".sqlite") {
            self.parse_primary_sqlite(&decompressed)?
        } else {
            self.parse_primary_xml_stream(&decompressed)?
        };

        // Sort newest-first, dedup by filename, take top n
        packages.sort_by(|a, b| b.version.cmp(&a.version));
        packages.dedup_by(|a, b| a.filename == b.filename);
        packages.truncate(n);
        Ok(packages)
    }

    /// Fetch and parse `repodata/repomd.xml`.
    async fn fetch_repomd(&self) -> Result<Repomd> {
        let url = format!("{}/repodata/repomd.xml", self.url);
        let body = self
            .client
            .get(&url)
            .send()
            .await
            .context("Failed to fetch repomd.xml")?
            .error_for_status()
            .context("repomd.xml request error")?
            .text()
            .await
            .context("Failed to read repomd.xml body")?;

        let repomd: Repomd =
            quick_xml::de::from_str(&body).context("Failed to parse repomd.xml")?;
        Ok(repomd)
    }

    /// Find the primary metadata href in repomd.xml.
    /// Prefers `primary_db` (SQLite) over `primary` (XML) for faster filtered queries.
    fn find_primary_href(&self, repomd: &Repomd) -> Result<String> {
        // Prefer SQLite database
        if let Some(db) = repomd.entries.iter().find(|d| d.data_type == "primary_db") {
            return Ok(db.location.href.clone());
        }
        // Fall back to XML
        if let Some(xml) = repomd.entries.iter().find(|d| d.data_type == "primary") {
            return Ok(xml.location.href.clone());
        }
        bail!("repomd.xml contains no primary or primary_db metadata entry")
    }

    /// Fetch raw bytes of a metadata file (relative to repo base URL).
    async fn fetch_metadata_bytes(&self, href: &str) -> Result<Vec<u8>> {
        let url = format!("{}/{}", self.url, href.trim_start_matches('/'));
        let bytes = self
            .client
            .get(&url)
            .send()
            .await
            .with_context(|| format!("Failed to fetch metadata: {url}"))?
            .error_for_status()
            .with_context(|| format!("Metadata request error: {url}"))?
            .bytes()
            .await
            .with_context(|| format!("Failed to read metadata body: {url}"))?;
        Ok(bytes.to_vec())
    }

    /// Decompress metadata based on file extension.
    fn decompress(bytes: &[u8], href: &str) -> Result<Vec<u8>> {
        if href.ends_with(".gz") {
            let mut decoder = flate2::read::GzDecoder::new(bytes);
            let mut out = Vec::new();
            decoder
                .read_to_end(&mut out)
                .context("Failed to decompress gzip metadata")?;
            Ok(out)
        } else if href.ends_with(".xz") {
            let mut out = Vec::new();
            lzma_rs::xz_decompress(&mut &bytes[..], &mut out)
                .context("Failed to decompress XZ metadata")?;
            Ok(out)
        } else if href.ends_with(".bz2") {
            bail!("bz2 compression is not currently supported for RPM metadata; use .gz or .xz")
        } else {
            // Assume uncompressed
            Ok(bytes.to_vec())
        }
    }

    /// Stream-parse `primary.xml` without loading the full DOM into memory.
    fn parse_primary_xml_stream(&self, xml: &[u8]) -> Result<Vec<RemotePackage>> {
        let mut reader = Reader::from_reader(xml);
        reader.config_mut().trim_text(true);

        let mut packages = Vec::new();
        let mut buf = Vec::new();

        // Per-package state
        let mut in_package = false;
        let mut name = String::new();
        let mut arch = String::new();
        let mut epoch: u32 = 0;
        let mut ver = String::new();
        let mut rel = String::new();
        let mut location_href = String::new();
        let mut reading_name = false;
        let mut reading_arch = false;

        loop {
            match reader.read_event_into(&mut buf) {
                Ok(Event::Start(ref e)) => match e.name().as_ref() {
                    b"package" => {
                        in_package = true;
                        name.clear();
                        arch.clear();
                        ver.clear();
                        rel.clear();
                        location_href.clear();
                        epoch = 0;
                    }
                    b"name" if in_package => reading_name = true,
                    b"arch" if in_package => reading_arch = true,
                    _ => {}
                },
                Ok(Event::Empty(ref e)) if in_package => match e.name().as_ref() {
                    b"version" => {
                        for attr in e.attributes().flatten() {
                            match attr.key.as_ref() {
                                b"epoch" => {
                                    epoch = std::str::from_utf8(&attr.value)
                                        .unwrap_or("0")
                                        .parse()
                                        .unwrap_or(0);
                                }
                                b"ver" => {
                                    ver = String::from_utf8_lossy(&attr.value).into_owned();
                                }
                                b"rel" => {
                                    rel = String::from_utf8_lossy(&attr.value).into_owned();
                                }
                                _ => {}
                            }
                        }
                    }
                    b"location" => {
                        for attr in e.attributes().flatten() {
                            if attr.key.as_ref() == b"href" {
                                location_href = String::from_utf8_lossy(&attr.value).into_owned();
                            }
                        }
                    }
                    _ => {}
                },
                Ok(Event::Text(ref e)) => {
                    if reading_name {
                        name = e.unescape().unwrap_or_default().into_owned();
                        reading_name = false;
                    }
                    if reading_arch {
                        arch = e.unescape().unwrap_or_default().into_owned();
                        reading_arch = false;
                    }
                }
                Ok(Event::End(ref e)) => match e.name().as_ref() {
                    b"name" => reading_name = false,
                    b"arch" => reading_arch = false,
                    b"package" if in_package => {
                        in_package = false;
                        let entry = RpmPackageEntry {
                            name: name.clone(),
                            arch: arch.clone(),
                            epoch,
                            version: ver.clone(),
                            release: rel.clone(),
                            location_href: location_href.clone(),
                        };
                        if self.apply_filters(&entry) {
                            packages.push(entry.to_remote_package(&self.url));
                        }
                    }
                    _ => {}
                },
                Ok(Event::Eof) => break,
                Err(e) => bail!("Failed to parse primary.xml: {e}"),
                _ => {}
            }
            buf.clear();
        }
        Ok(packages)
    }

    /// Parse packages from a `primary.sqlite` database.
    fn parse_primary_sqlite(&self, db_bytes: &[u8]) -> Result<Vec<RemotePackage>> {
        let tmp = tempfile::NamedTempFile::new()
            .context("Failed to create temp file for SQLite database")?;
        std::fs::write(tmp.path(), db_bytes).context("Failed to write SQLite database")?;

        let conn = rusqlite::Connection::open_with_flags(
            tmp.path(),
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .context("Failed to open primary.sqlite database")?;

        let mut stmt = conn
            .prepare("SELECT name, arch, epoch, version, \"release\", location_href FROM packages")
            .context("Failed to prepare SQLite query")?;

        let mut packages = Vec::new();
        let rows = stmt
            .query_map([], |row| {
                Ok(RpmPackageEntry {
                    name: row.get(0)?,
                    arch: row.get(1)?,
                    epoch: row.get::<_, String>(2)?.parse().unwrap_or(0),
                    version: row.get(3)?,
                    release: row.get(4)?,
                    location_href: row.get(5)?,
                })
            })
            .context("Failed to query packages from SQLite")?;

        for row in rows {
            let entry = row.context("Failed to read package row")?;
            if self.apply_filters(&entry) {
                packages.push(entry.to_remote_package(&self.url));
            }
        }

        Ok(packages)
    }

    /// Check if a package entry passes all configured filters.
    fn apply_filters(&self, entry: &RpmPackageEntry) -> bool {
        // Architecture filter
        if !self.architectures.iter().any(|a| a == &entry.arch) {
            return false;
        }
        // Package name filter
        if !self.package_filter.is_empty() && !self.package_filter.iter().any(|f| f == &entry.name)
        {
            return false;
        }
        // Filename glob filter
        if let Some(ref pattern) = self.filename_filter {
            let basename = entry
                .location_href
                .rsplit('/')
                .next()
                .unwrap_or(&entry.location_href);
            if !pattern.matches(basename) {
                return false;
            }
        }
        true
    }

    /// Verify the GPG signature of `repomd.xml` using `repomd.xml.asc`.
    async fn verify_repomd_gpg(&self) -> Result<()> {
        let Some(ref key_source) = self.gpg_key else {
            debug!("GPG verification enabled but no gpg_key set; skipping");
            return Ok(());
        };

        // Check that the gpg binary is available
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

        // Resolve key material
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

        // Fetch repomd.xml and its detached signature
        let repomd_url = format!("{}/repodata/repomd.xml", self.url);
        let sig_url = format!("{}/repodata/repomd.xml.asc", self.url);

        let repomd_body = self
            .client
            .get(&repomd_url)
            .send()
            .await
            .context("Failed to fetch repomd.xml for GPG verification")?
            .error_for_status()
            .context("repomd.xml request error")?
            .bytes()
            .await
            .context("Failed to read repomd.xml body")?;

        let sig_body = self
            .client
            .get(&sig_url)
            .send()
            .await
            .context("Failed to fetch repomd.xml.asc")?
            .error_for_status()
            .context("repomd.xml.asc request error")?
            .bytes()
            .await
            .context("Failed to read repomd.xml.asc body")?;

        // Write to temp files, dearmor key, verify detached signature
        let tmp = tempfile::tempdir().context("Failed to create temp dir for GPG")?;
        let keyring = tmp.path().join("repo.gpg");
        let repomd_file = tmp.path().join("repomd.xml");
        let sig_file = tmp.path().join("repomd.xml.asc");
        let dearmored = tmp.path().join("repo-dearmored.gpg");

        std::fs::write(&keyring, &key_data).context("Failed to write GPG keyring")?;
        std::fs::write(&repomd_file, &repomd_body).context("Failed to write repomd.xml")?;
        std::fs::write(&sig_file, &sig_body).context("Failed to write repomd.xml.asc")?;

        // Dearmor the key
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
            .context("Failed to run gpg --dearmor")?;

        if !dearmor_out.status.success() {
            bail!(
                "gpg --dearmor failed: {}",
                String::from_utf8_lossy(&dearmor_out.stderr)
            );
        }

        // Verify detached signature
        let verify_out = std::process::Command::new("gpg")
            .args([
                "--homedir",
                tmp.path().to_str().unwrap(),
                "--no-default-keyring",
                "--keyring",
                dearmored.to_str().unwrap(),
                "--verify",
                sig_file.to_str().unwrap(),
                repomd_file.to_str().unwrap(),
            ])
            .output()
            .context("Failed to run gpg --verify")?;

        if verify_out.status.success() {
            debug!("GPG signature verified for repomd.xml");
            Ok(())
        } else {
            bail!(
                "GPG signature verification failed for repomd.xml: {}",
                String::from_utf8_lossy(&verify_out.stderr)
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::{MockResponse, MockServer};

    fn source(url: &str) -> RpmRepoSource {
        RpmRepoSource::new(
            url,
            vec![],
            None,
            false,
            None,
            vec!["x86_64".to_string(), "noarch".to_string()],
        )
        .unwrap()
    }

    fn sample_repomd(primary_href: &str) -> String {
        format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<repomd>
  <data type="primary">
    <location href="{primary_href}"/>
  </data>
</repomd>"#
        )
    }

    fn sample_repomd_with_sqlite(sqlite_href: &str) -> String {
        format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<repomd>
  <data type="primary">
    <location href="repodata/primary.xml.gz"/>
  </data>
  <data type="primary_db">
    <location href="{sqlite_href}"/>
  </data>
</repomd>"#
        )
    }

    fn sample_primary_xml(packages: &[(&str, &str, u32, &str, &str, &str)]) -> String {
        let mut xml = String::from(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<metadata xmlns="http://linux.duke.edu/metadata/common" xmlns:rpm="http://linux.duke.edu/metadata/rpm" packages="0">
"#,
        );
        for (name, arch, epoch, ver, rel, href) in packages {
            xml.push_str(&format!(
                r#"<package type="rpm">
  <name>{name}</name>
  <arch>{arch}</arch>
  <version epoch="{epoch}" ver="{ver}" rel="{rel}"/>
  <location href="{href}"/>
</package>
"#
            ));
        }
        xml.push_str("</metadata>");
        xml
    }

    // ── Unit tests ─────────────────────────────────────────────────────────

    #[test]
    fn parse_repomd_extracts_primary_location() {
        let xml = sample_repomd("repodata/abc-primary.xml.gz");
        let repomd: Repomd = quick_xml::de::from_str(&xml).unwrap();
        let s = source("https://example.com");
        let href = s.find_primary_href(&repomd).unwrap();
        assert_eq!(href, "repodata/abc-primary.xml.gz");
    }

    #[test]
    fn parse_repomd_prefers_sqlite_over_xml() {
        let xml = sample_repomd_with_sqlite("repodata/abc-primary.sqlite.gz");
        let repomd: Repomd = quick_xml::de::from_str(&xml).unwrap();
        let s = source("https://example.com");
        let href = s.find_primary_href(&repomd).unwrap();
        assert_eq!(href, "repodata/abc-primary.sqlite.gz");
    }

    #[test]
    fn parse_repomd_missing_primary_fails() {
        let xml = r#"<?xml version="1.0"?><repomd><data type="filelists"><location href="x"/></data></repomd>"#;
        let repomd: Repomd = quick_xml::de::from_str(xml).unwrap();
        let s = source("https://example.com");
        assert!(s.find_primary_href(&repomd).is_err());
    }

    #[test]
    fn parse_primary_single_package() {
        let xml = sample_primary_xml(&[(
            "nginx",
            "x86_64",
            0,
            "1.24.0",
            "1.el9",
            "Packages/n/nginx-1.24.0-1.el9.x86_64.rpm",
        )]);
        let s = source("https://repo.example.com");
        let pkgs = s.parse_primary_xml_stream(xml.as_bytes()).unwrap();
        assert_eq!(pkgs.len(), 1);
        assert_eq!(pkgs[0].filename, "nginx-1.24.0-1.el9.x86_64.rpm");
        assert_eq!(pkgs[0].version, PackageVersion::parse("1.24.0-1.el9"));
        assert_eq!(
            pkgs[0].download_url,
            "https://repo.example.com/Packages/n/nginx-1.24.0-1.el9.x86_64.rpm"
        );
    }

    #[test]
    fn parse_primary_with_epoch_nonzero() {
        let xml = sample_primary_xml(&[(
            "vim",
            "x86_64",
            2,
            "9.0.1",
            "1.el9",
            "Packages/v/vim-9.0.1-1.el9.x86_64.rpm",
        )]);
        let s = source("https://repo.example.com");
        let pkgs = s.parse_primary_xml_stream(xml.as_bytes()).unwrap();
        assert_eq!(pkgs.len(), 1);
        assert_eq!(pkgs[0].version, PackageVersion::parse("2:9.0.1-1.el9"));
    }

    #[test]
    fn parse_primary_with_epoch_zero_excluded() {
        let xml = sample_primary_xml(&[(
            "curl",
            "x86_64",
            0,
            "7.88.1",
            "2.el9",
            "Packages/c/curl-7.88.1-2.el9.x86_64.rpm",
        )]);
        let s = source("https://repo.example.com");
        let pkgs = s.parse_primary_xml_stream(xml.as_bytes()).unwrap();
        assert_eq!(pkgs[0].version, PackageVersion::parse("7.88.1-2.el9"));
    }

    #[test]
    fn architecture_filter_includes_noarch() {
        let xml = sample_primary_xml(&[
            (
                "nginx",
                "x86_64",
                0,
                "1.24.0",
                "1.el9",
                "Packages/nginx.rpm",
            ),
            (
                "nginx-docs",
                "noarch",
                0,
                "1.24.0",
                "1.el9",
                "Packages/nginx-docs.rpm",
            ),
            (
                "nginx-arm",
                "aarch64",
                0,
                "1.24.0",
                "1.el9",
                "Packages/nginx-arm.rpm",
            ),
        ]);
        let s = source("https://repo.example.com");
        let pkgs = s.parse_primary_xml_stream(xml.as_bytes()).unwrap();
        assert_eq!(pkgs.len(), 2); // x86_64 + noarch, not aarch64
        assert!(pkgs.iter().any(|p| p.filename == "nginx.rpm"));
        assert!(pkgs.iter().any(|p| p.filename == "nginx-docs.rpm"));
    }

    #[test]
    fn package_filter_exact_name() {
        let xml = sample_primary_xml(&[
            (
                "nginx",
                "x86_64",
                0,
                "1.24.0",
                "1.el9",
                "Packages/nginx.rpm",
            ),
            ("curl", "x86_64", 0, "7.88.1", "2.el9", "Packages/curl.rpm"),
        ]);
        let mut s = source("https://repo.example.com");
        s.package_filter = vec!["nginx".to_string()];
        let pkgs = s.parse_primary_xml_stream(xml.as_bytes()).unwrap();
        assert_eq!(pkgs.len(), 1);
        assert_eq!(pkgs[0].filename, "nginx.rpm");
    }

    #[test]
    fn filename_filter_glob() {
        let xml = sample_primary_xml(&[
            (
                "nginx",
                "x86_64",
                0,
                "1.24.0",
                "1.el9",
                "Packages/nginx-1.24.0-1.el9.x86_64.rpm",
            ),
            (
                "nginx-mod",
                "x86_64",
                0,
                "1.24.0",
                "1.el9",
                "Packages/nginx-mod-1.24.0-1.el9.x86_64.rpm",
            ),
        ]);
        let mut s = source("https://repo.example.com");
        s.filename_filter = Some(glob::Pattern::new("nginx-1*").unwrap());
        let pkgs = s.parse_primary_xml_stream(xml.as_bytes()).unwrap();
        assert_eq!(pkgs.len(), 1);
        assert_eq!(pkgs[0].filename, "nginx-1.24.0-1.el9.x86_64.rpm");
    }

    #[test]
    fn invalid_filename_filter_rejected() {
        let err = RpmRepoSource::new(
            "https://example.com",
            vec![],
            Some("[bad"),
            false,
            None,
            vec!["x86_64".to_string()],
        )
        .unwrap_err();
        assert!(err.to_string().contains("Invalid filename_filter"));
    }

    // ── Async integration tests with mock server ───────────────────────────

    #[tokio::test]
    async fn fetch_latest_from_mock_repo_xml() {
        let primary_xml = sample_primary_xml(&[
            (
                "nginx",
                "x86_64",
                0,
                "1.26.0",
                "1.el9",
                "Packages/n/nginx-1.26.0-1.el9.x86_64.rpm",
            ),
            (
                "nginx",
                "x86_64",
                0,
                "1.24.0",
                "1.el9",
                "Packages/n/nginx-1.24.0-1.el9.x86_64.rpm",
            ),
        ]);

        // Compress with gzip
        use flate2::Compression;
        use flate2::write::GzEncoder;
        use std::io::Write;
        let mut enc = GzEncoder::new(Vec::new(), Compression::default());
        enc.write_all(primary_xml.as_bytes()).unwrap();
        let gz_bytes = enc.finish().unwrap();

        let repomd = sample_repomd("repodata/primary.xml.gz");

        let server = MockServer::start(vec![
            MockResponse::json(200, &repomd), // repomd.xml
            MockResponse::bytes(200, gz_bytes, &[("Content-Type", "application/gzip")]), // primary.xml.gz
        ]);
        let s = source("placeholder").with_url(&server.url);

        let pkgs = s.fetch_latest(10).await.unwrap();
        assert_eq!(pkgs.len(), 2);
        assert_eq!(pkgs[0].filename, "nginx-1.26.0-1.el9.x86_64.rpm");
        assert_eq!(pkgs[1].filename, "nginx-1.24.0-1.el9.x86_64.rpm");
    }

    #[tokio::test]
    async fn fetch_latest_truncates_to_n() {
        let primary_xml = sample_primary_xml(&[
            ("a", "x86_64", 0, "3.0.0", "1.el9", "Packages/a-3.rpm"),
            ("a", "x86_64", 0, "2.0.0", "1.el9", "Packages/a-2.rpm"),
            ("a", "x86_64", 0, "1.0.0", "1.el9", "Packages/a-1.rpm"),
        ]);

        use flate2::Compression;
        use flate2::write::GzEncoder;
        use std::io::Write;
        let mut enc = GzEncoder::new(Vec::new(), Compression::default());
        enc.write_all(primary_xml.as_bytes()).unwrap();
        let gz_bytes = enc.finish().unwrap();

        let repomd = sample_repomd("repodata/primary.xml.gz");
        let server = MockServer::start(vec![
            MockResponse::json(200, &repomd),
            MockResponse::bytes(200, gz_bytes, &[("Content-Type", "application/gzip")]),
        ]);
        let s = source("placeholder").with_url(&server.url);

        let pkgs = s.fetch_latest(2).await.unwrap();
        assert_eq!(pkgs.len(), 2);
    }

    #[tokio::test]
    async fn fetch_latest_http_error_fails() {
        let server = MockServer::start(vec![MockResponse::json(500, "error")]);
        let s = source("placeholder").with_url(&server.url);

        let err = s.fetch_latest(10).await.unwrap_err();
        assert!(
            err.to_string().contains("repomd.xml") || err.to_string().contains("500"),
            "unexpected error: {err}"
        );
    }

    #[tokio::test]
    async fn fetch_latest_sqlite_primary() {
        // Create a minimal SQLite database
        let tmp = tempfile::tempdir().unwrap();
        let db_path = tmp.path().join("primary.sqlite");
        {
            let conn = rusqlite::Connection::open(&db_path).unwrap();
            conn.execute_batch(
                "CREATE TABLE packages (
                    name TEXT, arch TEXT, epoch TEXT, version TEXT,
                    \"release\" TEXT, location_href TEXT
                )",
            )
            .unwrap();
            conn.execute(
                "INSERT INTO packages VALUES ('nginx', 'x86_64', '0', '1.24.0', '1.el9', 'Packages/nginx-1.24.0-1.el9.x86_64.rpm')",
                [],
            )
            .unwrap();
        }
        let db_bytes = std::fs::read(&db_path).unwrap();

        // Compress with gzip
        use flate2::Compression;
        use flate2::write::GzEncoder;
        use std::io::Write;
        let mut enc = GzEncoder::new(Vec::new(), Compression::default());
        enc.write_all(&db_bytes).unwrap();
        let gz_bytes = enc.finish().unwrap();

        let repomd = sample_repomd_with_sqlite("repodata/primary.sqlite.gz");
        let server = MockServer::start(vec![
            MockResponse::json(200, &repomd),
            MockResponse::bytes(200, gz_bytes, &[("Content-Type", "application/gzip")]),
        ]);
        let s = source("placeholder").with_url(&server.url);

        let pkgs = s.fetch_latest(10).await.unwrap();
        assert_eq!(pkgs.len(), 1);
        assert_eq!(pkgs[0].filename, "nginx-1.24.0-1.el9.x86_64.rpm");
        assert_eq!(pkgs[0].version, PackageVersion::parse("1.24.0-1.el9"));
    }
}
