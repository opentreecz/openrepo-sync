use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::time::Duration;

/// Deserializes a YAML field that may be either a single string or a sequence
/// of strings into `Vec<String>`. Also accepts missing/null (returns empty vec).
///
/// YAML examples that all work:
/// ```yaml
/// arch_filter: amd64
/// arch_filter: [amd64, arm64]
/// arch_filter:
///   - amd64
///   - arm64
/// ```
fn deserialize_string_or_list<'de, D>(deserializer: D) -> Result<Vec<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum StringOrList {
        Single(String),
        List(Vec<String>),
    }

    let opt: Option<StringOrList> = Option::deserialize(deserializer)?;
    Ok(match opt {
        None => vec![],
        Some(StringOrList::Single(s)) => vec![s],
        Some(StringOrList::List(v)) => v,
    })
}

/// Returns the default arch priority list: amd64 first, then arm64.
fn default_arch_filter() -> Vec<String> {
    vec!["amd64".to_string(), "arm64".to_string()]
}

fn default_deb_suites() -> Vec<String> {
    vec!["bookworm".to_string()]
}

fn default_deb_components() -> Vec<String> {
    vec!["main".to_string()]
}

fn default_deb_architectures() -> Vec<String> {
    vec!["amd64".to_string()]
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Deserialize)]
pub struct GlobalConfig {
    pub openrepo: OpenRepoConfig,
    #[serde(default = "default_download_dir")]
    pub download_dir: PathBuf,
    #[serde(default)]
    pub schedule: ScheduleConfig,
}

fn default_download_dir() -> PathBuf {
    std::env::temp_dir().join("openrepo-sync")
}

#[derive(Debug, Clone, Deserialize)]
pub struct OpenRepoConfig {
    pub api_url: String,
    pub api_key: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ScheduleConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_schedule_interval")]
    pub interval: String,
    #[serde(default = "default_true")]
    pub run_on_start: bool,
}

impl Default for ScheduleConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            interval: default_schedule_interval(),
            run_on_start: true,
        }
    }
}

fn default_schedule_interval() -> String {
    "24h".to_string()
}

impl ScheduleConfig {
    pub fn interval_duration(&self) -> Result<Duration> {
        parse_interval(&self.interval)
    }
}

fn parse_interval(value: &str) -> Result<Duration> {
    let value = value.trim();
    if value.len() < 2 {
        anyhow::bail!("Invalid schedule interval '{value}'; expected format like 30m, 6h, or 1d");
    }

    let (amount, unit) = value.split_at(value.len() - 1);
    let amount: u64 = amount.parse().with_context(|| {
        format!("Invalid schedule interval '{value}'; expected format like 30m, 6h, or 1d")
    })?;

    if amount == 0 {
        anyhow::bail!("Invalid schedule interval '{value}'; interval must be greater than zero");
    }

    let seconds = match unit {
        "m" => amount * 60,
        "h" => amount * 60 * 60,
        "d" => amount * 24 * 60 * 60,
        _ => anyhow::bail!("Invalid schedule interval '{value}'; supported units are m, h, and d"),
    };

    Ok(Duration::from_secs(seconds))
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Default)]
#[serde(rename_all = "snake_case")]
pub enum OnConflict {
    /// Return an error if the package already exists (default).
    #[default]
    Error,
    /// Skip uploading if the package already exists.
    Skip,
    /// Overwrite the existing package.
    Overwrite,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Default)]
#[serde(rename_all = "snake_case")]
pub enum DebRepoLayout {
    /// Standard Debian repository layout: dists/<suite>/<component>/binary-<arch>/Packages.
    #[default]
    Debian,
    /// Flat APT repository layout used by OBS: Packages at the repository root.
    Flat,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ProjectConfig {
    pub name: String,
    pub repo_uid: String,
    pub keep_versions: usize,
    #[serde(default)]
    pub on_conflict: OnConflict,
    pub source: SourceConfig,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SourceConfig {
    Github {
        owner: String,
        repo: String,
        #[serde(default)]
        asset_filter: Option<String>,
        #[serde(default)]
        prerelease: bool,
        /// Ordered architecture preference list. The first entry that matches
        /// an asset filename is selected. When a release has assets for multiple
        /// architectures, this prevents accidentally downloading the wrong one.
        /// Default: ["amd64", "arm64"]. Set to [] to disable arch filtering.
        /// Accepts a single string or a list: `arch_filter: amd64`
        #[serde(
            default = "default_arch_filter",
            deserialize_with = "deserialize_string_or_list"
        )]
        arch_filter: Vec<String>,
    },
    DirectUrl {
        url: String,
    },
    DirectUrlLatest {
        url: String,
    },
    Sourceforge {
        project: String,
        #[serde(default)]
        folder: Option<String>,
        #[serde(default)]
        filename_filter: Option<String>,
    },
    DebRepo {
        url: String,
        /// Repository metadata layout. Default: standard Debian dists/ layout.
        #[serde(default)]
        layout: DebRepoLayout,
        /// Debian suite(s) to fetch, e.g. "bookworm" or ["bookworm", "bullseye"].
        #[serde(
            default = "default_deb_suites",
            deserialize_with = "deserialize_string_or_list"
        )]
        suites: Vec<String>,
        /// Repository component(s), e.g. "main" or ["main", "contrib"].
        #[serde(
            default = "default_deb_components",
            deserialize_with = "deserialize_string_or_list"
        )]
        components: Vec<String>,
        /// Architecture(s) to mirror, e.g. "amd64" or ["amd64", "arm64"].
        #[serde(
            default = "default_deb_architectures",
            deserialize_with = "deserialize_string_or_list"
        )]
        architectures: Vec<String>,
        /// Exact Debian package name(s) to sync (Package: field). Required unless
        /// filename_filter is set. Accepts a single string or a list.
        #[serde(default, deserialize_with = "deserialize_string_or_list")]
        package_filter: Vec<String>,
        /// Optional glob filter applied to the Filename field in the Packages index.
        #[serde(default)]
        filename_filter: Option<String>,
        /// Verify the repository's InRelease/Release GPG signature. Default: true.
        #[serde(default = "default_true")]
        verify_gpg: bool,
        /// GPG public key for signature verification. Either an inline ASCII-armored
        /// key or a URL (http/https) that will be fetched at sync time.
        /// Required when verify_gpg is true.
        #[serde(default)]
        gpg_key: Option<String>,
    },
}

impl GlobalConfig {
    pub fn load(path: &std::path::Path) -> Result<Self> {
        let content = std::fs::read_to_string(path)
            .with_context(|| format!("Failed to read config file: {}", path.display()))?;
        let content = expand_env_vars(&content);
        let config: GlobalConfig = serde_yaml::from_str(&content)
            .with_context(|| format!("Failed to parse config file: {}", path.display()))?;
        Ok(config)
    }
}

impl ProjectConfig {
    pub fn load_all(dir: &std::path::Path) -> Result<Vec<Self>> {
        let mut projects = Vec::new();
        let entries = std::fs::read_dir(dir)
            .with_context(|| format!("Failed to read projects directory: {}", dir.display()))?;
        for entry in entries {
            let entry = entry?;
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) == Some("yaml")
                || path.extension().and_then(|e| e.to_str()) == Some("yml")
            {
                let content = std::fs::read_to_string(&path)
                    .with_context(|| format!("Failed to read project file: {}", path.display()))?;
                let project: ProjectConfig = serde_yaml::from_str(&content)
                    .with_context(|| format!("Failed to parse project file: {}", path.display()))?;
                projects.push(project);
            }
        }
        projects.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(projects)
    }
}

fn expand_env_vars(s: &str) -> String {
    let re = regex::Regex::new(r"\$\{([^}]+)\}").unwrap();
    re.replace_all(s, |caps: &regex::Captures| {
        std::env::var(&caps[1]).unwrap_or_else(|_| caps[0].to_string())
    })
    .into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── GlobalConfig deserialization ───────────────────────────────────────

    #[test]
    fn global_config_minimal() {
        let yaml = r#"
openrepo:
  api_url: "https://repo.example.com"
  api_key: "tok123"
"#;
        let cfg: GlobalConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(cfg.openrepo.api_url, "https://repo.example.com");
        assert_eq!(cfg.openrepo.api_key, "tok123");
        // download_dir defaults to system temp + "openrepo-sync"
        assert!(cfg.download_dir.ends_with("openrepo-sync"));
        assert!(cfg.schedule.enabled);
        assert_eq!(cfg.schedule.interval, "24h");
        assert!(cfg.schedule.run_on_start);
    }

    #[test]
    fn global_config_explicit_download_dir() {
        let yaml = r#"
openrepo:
  api_url: "https://repo.example.com"
  api_key: "tok"
download_dir: "/var/cache/openrepo"
"#;
        let cfg: GlobalConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(
            cfg.download_dir,
            std::path::PathBuf::from("/var/cache/openrepo")
        );
    }

    #[test]
    fn global_config_explicit_schedule() {
        let yaml = r#"
openrepo:
  api_url: "https://repo.example.com"
  api_key: "tok"
schedule:
  enabled: true
  interval: "6h"
  run_on_start: false
"#;
        let cfg: GlobalConfig = serde_yaml::from_str(yaml).unwrap();
        assert!(cfg.schedule.enabled);
        assert_eq!(cfg.schedule.interval, "6h");
        assert!(!cfg.schedule.run_on_start);
    }

    #[test]
    fn schedule_interval_parses_supported_units() {
        assert_eq!(parse_interval("30m").unwrap(), Duration::from_secs(30 * 60));
        assert_eq!(
            parse_interval("6h").unwrap(),
            Duration::from_secs(6 * 60 * 60)
        );
        assert_eq!(
            parse_interval("24h").unwrap(),
            Duration::from_secs(24 * 60 * 60)
        );
        assert_eq!(
            parse_interval("1d").unwrap(),
            Duration::from_secs(24 * 60 * 60)
        );
    }

    #[test]
    fn schedule_interval_rejects_invalid_values() {
        for value in ["", "0h", "tenm", "5s", "h"] {
            assert!(parse_interval(value).is_err(), "{value} should be invalid");
        }
    }

    // ── ProjectConfig deserialization ──────────────────────────────────────

    #[test]
    fn project_config_github() {
        let yaml = r#"
name: curl
repo_uid: debian-stable
keep_versions: 3
source:
  type: github
  owner: curl
  repo: curl
  asset_filter: "*.deb"
"#;
        let p: ProjectConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(p.name, "curl");
        assert_eq!(p.repo_uid, "debian-stable");
        assert_eq!(p.keep_versions, 3);
        assert!(matches!(p.source, SourceConfig::Github { .. }));
    }

    #[test]
    fn project_config_direct_url() {
        let yaml = r#"
name: tool
repo_uid: my-repo
keep_versions: 1
source:
  type: direct_url
  url: "https://example.com/tool-1.0.0.deb"
"#;
        let p: ProjectConfig = serde_yaml::from_str(yaml).unwrap();
        assert!(matches!(p.source, SourceConfig::DirectUrl { .. }));
    }

    #[test]
    fn project_config_direct_url_latest() {
        let yaml = r#"
name: tool
repo_uid: my-repo
keep_versions: 1
source:
  type: direct_url_latest
  url: "https://example.com/tool-LATEST.deb"
"#;
        let p: ProjectConfig = serde_yaml::from_str(yaml).unwrap();
        assert!(matches!(p.source, SourceConfig::DirectUrlLatest { .. }));
    }

    #[test]
    fn project_config_sourceforge() {
        let yaml = r#"
name: sfpkg
repo_uid: sf-repo
keep_versions: 2
source:
  type: sourceforge
  project: my-project
  folder: "releases/linux"
  filename_filter: "*.deb"
"#;
        let p: ProjectConfig = serde_yaml::from_str(yaml).unwrap();
        assert!(matches!(
            p.source,
            SourceConfig::Sourceforge {
                folder: Some(_),
                ..
            }
        ));
    }

    #[test]
    fn project_config_github_defaults() {
        let yaml = r#"
name: tool
repo_uid: r
keep_versions: 1
source:
  type: github
  owner: acme
  repo: tool
"#;
        let p: ProjectConfig = serde_yaml::from_str(yaml).unwrap();
        if let SourceConfig::Github {
            asset_filter,
            prerelease,
            arch_filter,
            ..
        } = p.source
        {
            assert!(asset_filter.is_none());
            assert!(!prerelease);
            assert_eq!(arch_filter, vec!["amd64", "arm64"]);
        } else {
            panic!("wrong variant");
        }
    }

    #[test]
    fn project_config_github_explicit_arch_filter() {
        let yaml = r#"
name: tool
repo_uid: r
keep_versions: 1
source:
  type: github
  owner: acme
  repo: tool
  arch_filter: ["arm64", "amd64"]
"#;
        let p: ProjectConfig = serde_yaml::from_str(yaml).unwrap();
        if let SourceConfig::Github { arch_filter, .. } = p.source {
            assert_eq!(arch_filter, vec!["arm64", "amd64"]);
        } else {
            panic!("wrong variant");
        }
    }

    #[test]
    fn project_config_github_empty_arch_filter_disables_selection() {
        let yaml = r#"
name: tool
repo_uid: r
keep_versions: 1
source:
  type: github
  owner: acme
  repo: tool
  arch_filter: []
"#;
        let p: ProjectConfig = serde_yaml::from_str(yaml).unwrap();
        if let SourceConfig::Github { arch_filter, .. } = p.source {
            assert!(arch_filter.is_empty());
        } else {
            panic!("wrong variant");
        }
    }

    #[test]
    fn project_config_github_arch_filter_single_string() {
        let yaml = r#"
name: tool
repo_uid: r
keep_versions: 1
source:
  type: github
  owner: acme
  repo: tool
  arch_filter: arm64
"#;
        let p: ProjectConfig = serde_yaml::from_str(yaml).unwrap();
        if let SourceConfig::Github { arch_filter, .. } = p.source {
            assert_eq!(arch_filter, vec!["arm64"]);
        } else {
            panic!("wrong variant");
        }
    }

    // ── DebRepo deserialization ────────────────────────────────────────────

    #[test]
    fn project_config_deb_repo_defaults() {
        let yaml = r#"
name: nginx
repo_uid: my-repo
keep_versions: 3
source:
  type: deb_repo
  url: "https://nginx.org/packages/debian"
  package_filter: nginx
"#;
        let p: ProjectConfig = serde_yaml::from_str(yaml).unwrap();
        if let SourceConfig::DebRepo {
            url,
            layout,
            suites,
            components,
            architectures,
            package_filter,
            filename_filter,
            verify_gpg,
            gpg_key,
        } = p.source
        {
            assert_eq!(url, "https://nginx.org/packages/debian");
            assert_eq!(layout, DebRepoLayout::Debian);
            assert_eq!(suites, vec!["bookworm"]);
            assert_eq!(components, vec!["main"]);
            assert_eq!(architectures, vec!["amd64"]);
            assert_eq!(package_filter, vec!["nginx"]);
            assert!(filename_filter.is_none());
            assert!(verify_gpg);
            assert!(gpg_key.is_none());
        } else {
            panic!("wrong variant");
        }
    }

    #[test]
    fn project_config_deb_repo_explicit_fields() {
        let yaml = r#"
name: nginx
repo_uid: my-repo
keep_versions: 2
source:
  type: deb_repo
  url: "https://nginx.org/packages/debian"
  layout: flat
  suites: [bookworm, bullseye]
  components: [main, nginx]
  architectures: [amd64, arm64]
  package_filter: [nginx, libnginx-mod-http-js]
  filename_filter: "nginx_*.deb"
  verify_gpg: false
  gpg_key: "https://nginx.org/keys/nginx_signing.key"
"#;
        let p: ProjectConfig = serde_yaml::from_str(yaml).unwrap();
        if let SourceConfig::DebRepo {
            layout,
            suites,
            components,
            architectures,
            package_filter,
            filename_filter,
            verify_gpg,
            gpg_key,
            ..
        } = p.source
        {
            assert_eq!(layout, DebRepoLayout::Flat);
            assert_eq!(suites, vec!["bookworm", "bullseye"]);
            assert_eq!(components, vec!["main", "nginx"]);
            assert_eq!(architectures, vec!["amd64", "arm64"]);
            assert_eq!(package_filter, vec!["nginx", "libnginx-mod-http-js"]);
            assert_eq!(filename_filter.as_deref(), Some("nginx_*.deb"));
            assert!(!verify_gpg);
            assert_eq!(
                gpg_key.as_deref(),
                Some("https://nginx.org/keys/nginx_signing.key")
            );
        } else {
            panic!("wrong variant");
        }
    }

    #[test]
    fn project_config_deb_repo_empty_package_filter_is_allowed() {
        let yaml = r#"
name: repo
repo_uid: r
keep_versions: 1
source:
  type: deb_repo
  url: "https://example.com"
  package_filter: []
"#;
        let p: ProjectConfig = serde_yaml::from_str(yaml).unwrap();
        if let SourceConfig::DebRepo { package_filter, .. } = p.source {
            assert!(package_filter.is_empty());
        } else {
            panic!("wrong variant");
        }
    }

    #[test]
    fn project_config_deb_repo_single_string_suites() {
        let yaml = r#"
name: nginx
repo_uid: r
keep_versions: 1
source:
  type: deb_repo
  url: "https://example.com"
  suites: bookworm
  components: main
  architectures: arm64
"#;
        let p: ProjectConfig = serde_yaml::from_str(yaml).unwrap();
        if let SourceConfig::DebRepo {
            suites,
            components,
            architectures,
            ..
        } = p.source
        {
            assert_eq!(suites, vec!["bookworm"]);
            assert_eq!(components, vec!["main"]);
            assert_eq!(architectures, vec!["arm64"]);
        } else {
            panic!("wrong variant");
        }
    }

    // ── env var expansion ──────────────────────────────────────────────────

    #[test]
    fn env_var_expansion_known_var() {
        // SAFETY: test binary is single-threaded at this point
        unsafe { std::env::set_var("TEST_OPENREPO_KEY", "secret42") };
        let result = super::expand_env_vars("api_key: ${TEST_OPENREPO_KEY}");
        assert_eq!(result, "api_key: secret42");
    }

    #[test]
    fn env_var_expansion_unknown_var_kept_as_is() {
        let result = super::expand_env_vars("api_key: ${SURELY_NOT_SET_XYZ}");
        assert_eq!(result, "api_key: ${SURELY_NOT_SET_XYZ}");
    }

    #[test]
    fn env_var_expansion_no_vars() {
        let result = super::expand_env_vars("plain string without vars");
        assert_eq!(result, "plain string without vars");
    }

    // ── GlobalConfig::load (filesystem) ────────────────────────────────────

    #[test]
    fn load_reads_file_and_expands_env_vars() {
        // SAFETY: no concurrent env access to this variable in the test binary
        unsafe { std::env::set_var("TEST_LOAD_API_KEY", "from-env") };
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.yaml");
        std::fs::write(
            &path,
            "openrepo:\n  api_url: \"https://repo.example.com\"\n  api_key: \"${TEST_LOAD_API_KEY}\"\n",
        )
        .unwrap();

        let cfg = GlobalConfig::load(&path).unwrap();
        assert_eq!(cfg.openrepo.api_key, "from-env");
    }

    #[test]
    fn load_missing_file_is_a_clear_error() {
        let err = GlobalConfig::load(std::path::Path::new("/nonexistent/config.yaml")).unwrap_err();
        assert!(err.to_string().contains("Failed to read config file"));
    }

    #[test]
    fn load_invalid_yaml_is_a_parse_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.yaml");
        std::fs::write(&path, "openrepo: [not, a, mapping").unwrap();
        let err = GlobalConfig::load(&path).unwrap_err();
        assert!(err.to_string().contains("Failed to parse config file"));
    }

    // ── ProjectConfig::load_all (filesystem) ───────────────────────────────

    fn write_project(dir: &std::path::Path, file: &str, name: &str) {
        std::fs::write(
            dir.join(file),
            format!(
                "name: {}\nrepo_uid: r\nkeep_versions: 1\nsource:\n  type: direct_url\n  url: \"https://example.com/x.deb\"\n",
                name
            ),
        )
        .unwrap();
    }

    #[test]
    fn load_all_reads_yaml_and_yml_sorted_by_name() {
        let dir = tempfile::tempdir().unwrap();
        write_project(dir.path(), "zeta.yaml", "zeta");
        write_project(dir.path(), "alpha.yml", "alpha");
        std::fs::write(dir.path().join("notes.txt"), "not a project").unwrap();

        let projects = ProjectConfig::load_all(dir.path()).unwrap();
        assert_eq!(projects.len(), 2);
        assert_eq!(projects[0].name, "alpha");
        assert_eq!(projects[1].name, "zeta");
    }

    #[test]
    fn load_all_missing_dir_is_a_clear_error() {
        let err =
            ProjectConfig::load_all(std::path::Path::new("/nonexistent/projects")).unwrap_err();
        assert!(
            err.to_string()
                .contains("Failed to read projects directory")
        );
    }

    #[test]
    fn load_all_invalid_project_file_is_a_parse_error() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("bad.yaml"), "name: [broken").unwrap();
        let err = ProjectConfig::load_all(dir.path()).unwrap_err();
        assert!(err.to_string().contains("Failed to parse project file"));
    }

    // ── OnConflict ─────────────────────────────────────────────────────────

    #[test]
    fn on_conflict_defaults_to_error() {
        let yaml = r#"
name: tool
repo_uid: r
keep_versions: 1
source:
  type: direct_url
  url: "https://example.com/x.deb"
"#;
        let p: ProjectConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(p.on_conflict, OnConflict::Error);
    }

    #[test]
    fn on_conflict_parses_snake_case_variants() {
        for (text, expected) in [
            ("skip", OnConflict::Skip),
            ("overwrite", OnConflict::Overwrite),
            ("error", OnConflict::Error),
        ] {
            let yaml = format!(
                "name: t\nrepo_uid: r\nkeep_versions: 1\non_conflict: {}\nsource:\n  type: direct_url\n  url: \"https://x/y.deb\"\n",
                text
            );
            let p: ProjectConfig = serde_yaml::from_str(&yaml).unwrap();
            assert_eq!(p.on_conflict, expected);
        }
    }
}
