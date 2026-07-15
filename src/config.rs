use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Deserialize)]
pub struct GlobalConfig {
    pub openrepo: OpenRepoConfig,
    #[serde(default = "default_download_dir")]
    pub download_dir: PathBuf,
}

fn default_download_dir() -> PathBuf {
    std::env::temp_dir().join("openrepo-sync")
}

#[derive(Debug, Clone, Deserialize)]
pub struct OpenRepoConfig {
    pub api_url: String,
    pub api_key: String,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum OnConflict {
    /// Return an error if the package already exists (default).
    Error,
    /// Skip uploading if the package already exists.
    Skip,
    /// Overwrite the existing package.
    Overwrite,
}

impl Default for OnConflict {
    fn default() -> Self {
        OnConflict::Error
    }
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
            ..
        } = p.source
        {
            assert!(asset_filter.is_none());
            assert!(!prerelease);
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
}
