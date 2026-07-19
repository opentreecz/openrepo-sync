mod config;
mod models;
mod repo_client;
mod sources;
mod sync;
#[cfg(test)]
mod test_util;
mod version;

use anyhow::{Context, Result};
use clap::{CommandFactory, Parser};
use std::path::PathBuf;
use tracing::{debug, info};
use tracing_subscriber::{EnvFilter, fmt};

#[derive(Parser)]
#[command(
    name = "openrepo-sync",
    version,
    about = "Sync packages from upstream sources into an OpenRepo repository",
    long_about = "openrepo-sync checks upstream sources (GitHub Releases, direct URLs, \
SourceForge) for new package versions, downloads them, uploads them to a \
self-hosted OpenRepo instance, and prunes old releases beyond a configured \
keep count.\n\nConfiguration is split into a global config file (server URL \
and API key) and per-project YAML files (one per software package).",
    after_help = "ENVIRONMENT:\n  OPENREPO_API_KEY   Override the api_key from config.yaml\n  \
RUST_LOG           Fine-grained log filter (e.g. RUST_LOG=openrepo=debug,reqwest=warn)"
)]
struct Cli {
    /// Path to the global config file
    #[arg(long, default_value = "config.yaml", value_name = "FILE")]
    config: PathBuf,

    /// Directory containing per-project YAML files
    #[arg(long, default_value = "projects", value_name = "DIR")]
    projects: PathBuf,

    /// Sync only this project (matched by name field in YAML)
    #[arg(long, value_name = "NAME")]
    project: Option<String>,

    /// Print what would happen without uploading or deleting anything
    #[arg(long)]
    dry_run: bool,

    /// Enable debug logging (equivalent to RUST_LOG=debug)
    #[arg(long, short)]
    verbose: bool,

    /// Generate a man page to stdout and exit
    #[arg(long, hide = true)]
    generate_man: bool,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    if cli.generate_man {
        generate_man_page()?;
        return Ok(());
    }

    init_logging(cli.verbose);

    let had_error = run(&cli).await?;
    if had_error {
        std::process::exit(1);
    }

    Ok(())
}

/// Load configuration, authenticate, and sync all selected projects.
/// Returns whether any project reported an error (the caller decides the
/// process exit code, keeping this testable).
async fn run(cli: &Cli) -> Result<bool> {
    debug!("Loading config from {}", cli.config.display());
    let global = config::GlobalConfig::load(&cli.config)
        .with_context(|| format!("Failed to load config from {}", cli.config.display()))?;
    debug!("OpenRepo API URL: {}", global.openrepo.api_url);
    debug!("Download directory: {}", global.download_dir.display());

    let client = repo_client::RepoClient::new(&global.openrepo.api_url, &global.openrepo.api_key)?;

    let username = client
        .whoami()
        .await
        .context("Authentication check failed — verify api_url and api_key in config")?;
    info!("Authenticated as: {}", username);

    debug!("Loading project configs from {}", cli.projects.display());
    let mut projects = config::ProjectConfig::load_all(&cli.projects)
        .with_context(|| format!("Failed to load projects from {}", cli.projects.display()))?;
    debug!("Loaded {} project(s)", projects.len());

    if let Some(ref name) = cli.project {
        projects.retain(|p| &p.name == name);
        if projects.is_empty() {
            anyhow::bail!(
                "No project named '{}' found in {}",
                name,
                cli.projects.display()
            );
        }
    }

    info!(
        "Syncing {} project(s){}",
        projects.len(),
        if cli.dry_run { " [dry-run]" } else { "" }
    );

    let mut had_error = false;
    for project in &projects {
        debug!(
            "[{}] Starting sync (repo_uid={})",
            project.name, project.repo_uid
        );
        let result = sync::sync_project(project, &client, &global.download_dir, cli.dry_run).await;
        for action in &result.actions {
            match action {
                models::SyncAction::UpToDate => {
                    info!("[{}] Up to date", result.project_name)
                }
                models::SyncAction::Uploaded { version } => {
                    info!("[{}] Uploaded version {}", result.project_name, version)
                }
                models::SyncAction::Skipped { version } => {
                    info!(
                        "[{}] Skipped version {} (already exists)",
                        result.project_name, version
                    )
                }
                models::SyncAction::Pruned { removed_count } => {
                    info!(
                        "[{}] Pruned {} old package(s)",
                        result.project_name, removed_count
                    )
                }
                models::SyncAction::Error(e) => {
                    eprintln!("[{}] ERROR: {}", result.project_name, e);
                    had_error = true;
                }
            }
        }
    }

    Ok(had_error)
}

fn init_logging(verbose: bool) {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| {
        if verbose {
            EnvFilter::new("debug")
        } else {
            EnvFilter::new("info")
        }
    });

    fmt()
        .with_env_filter(filter)
        .with_target(verbose) // show module path only in verbose mode
        .with_thread_ids(false)
        .with_file(false)
        .compact()
        .init();
}

fn generate_man_page() -> Result<()> {
    print!("{}", render_man_page()?);
    Ok(())
}

fn render_man_page() -> Result<String> {
    let cmd = Cli::command();
    let man = clap_mangen::Man::new(cmd);
    let mut buf = Vec::new();
    man.render(&mut buf)?;
    Ok(String::from_utf8(buf)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cli_definition_is_valid() {
        Cli::command().debug_assert();
    }

    #[test]
    fn cli_defaults() {
        let cli = Cli::try_parse_from(["openrepo-sync"]).unwrap();
        assert_eq!(cli.config, PathBuf::from("config.yaml"));
        assert_eq!(cli.projects, PathBuf::from("projects"));
        assert!(cli.project.is_none());
        assert!(!cli.dry_run);
        assert!(!cli.verbose);
        assert!(!cli.generate_man);
    }

    #[test]
    fn cli_parses_all_flags() {
        let cli = Cli::try_parse_from([
            "openrepo-sync",
            "--config",
            "/etc/openrepo/config.yaml",
            "--projects",
            "/etc/openrepo/projects",
            "--project",
            "curl",
            "--dry-run",
            "--verbose",
        ])
        .unwrap();
        assert_eq!(cli.config, PathBuf::from("/etc/openrepo/config.yaml"));
        assert_eq!(cli.projects, PathBuf::from("/etc/openrepo/projects"));
        assert_eq!(cli.project.as_deref(), Some("curl"));
        assert!(cli.dry_run);
        assert!(cli.verbose);
    }

    #[test]
    fn cli_short_verbose_flag() {
        let cli = Cli::try_parse_from(["openrepo-sync", "-v"]).unwrap();
        assert!(cli.verbose);
    }

    #[test]
    fn cli_rejects_unknown_flag() {
        assert!(Cli::try_parse_from(["openrepo-sync", "--bogus"]).is_err());
    }

    #[test]
    fn man_page_renders_with_command_name() {
        let man = render_man_page().unwrap();
        assert!(man.contains("openrepo-sync"));
        assert!(man.contains(".TH"), "expected roff output");
    }

    // ── run(): end-to-end over a mock OpenRepo server ──────────────────────

    use crate::test_util::{MockResponse, MockServer};

    /// Write a config.yaml + one direct_url project and return a Cli set up
    /// to use them in dry-run mode.
    fn setup_workspace(dir: &std::path::Path, api_url: &str) -> Cli {
        let config_path = dir.join("config.yaml");
        std::fs::write(
            &config_path,
            format!(
                "openrepo:\n  api_url: \"{}\"\n  api_key: \"test-key\"\ndownload_dir: \"{}\"\n",
                api_url,
                dir.join("downloads").display()
            ),
        )
        .unwrap();

        let projects_dir = dir.join("projects");
        std::fs::create_dir_all(&projects_dir).unwrap();
        std::fs::write(
            projects_dir.join("tool.yaml"),
            "name: tool\nrepo_uid: r\nkeep_versions: 5\nsource:\n  type: direct_url\n  url: \"https://example.com/tool-1.0.0.deb\"\n",
        )
        .unwrap();

        Cli {
            config: config_path,
            projects: projects_dir,
            project: None,
            dry_run: true,
            verbose: false,
            generate_man: false,
        }
    }

    #[tokio::test]
    async fn run_syncs_project_against_server_without_errors() {
        let server = MockServer::start(vec![
            MockResponse::json(200, r#"{"username":"alice"}"#), // whoami
            MockResponse::json(200, r#"{"results":[],"next":null}"#), // list
        ]);
        let dir = tempfile::tempdir().unwrap();
        let cli = setup_workspace(dir.path(), &server.url);

        let had_error = run(&cli).await.unwrap();
        assert!(!had_error);
    }

    #[tokio::test]
    async fn run_reports_error_when_sync_fails() {
        let server = MockServer::start(vec![
            MockResponse::json(200, r#"{"username":"alice"}"#), // whoami
            MockResponse::json(500, "boom"),                    // list fails
        ]);
        let dir = tempfile::tempdir().unwrap();
        let cli = setup_workspace(dir.path(), &server.url);

        let had_error = run(&cli).await.unwrap();
        assert!(had_error);
    }

    #[tokio::test]
    async fn run_filters_to_selected_project() {
        let server = MockServer::start(vec![
            MockResponse::json(200, r#"{"username":"alice"}"#),
            MockResponse::json(200, r#"{"results":[],"next":null}"#),
        ]);
        let dir = tempfile::tempdir().unwrap();
        let mut cli = setup_workspace(dir.path(), &server.url);
        cli.project = Some("tool".to_string());

        let had_error = run(&cli).await.unwrap();
        assert!(!had_error);
    }

    #[tokio::test]
    async fn run_fails_for_unknown_project_name() {
        let server = MockServer::start(vec![MockResponse::json(200, r#"{"username":"alice"}"#)]);
        let dir = tempfile::tempdir().unwrap();
        let mut cli = setup_workspace(dir.path(), &server.url);
        cli.project = Some("nope".to_string());

        let err = run(&cli).await.unwrap_err();
        assert!(err.to_string().contains("No project named 'nope'"));
    }

    #[tokio::test]
    async fn run_fails_for_missing_config() {
        let dir = tempfile::tempdir().unwrap();
        let cli = Cli {
            config: dir.path().join("missing.yaml"),
            projects: dir.path().join("projects"),
            project: None,
            dry_run: true,
            verbose: false,
            generate_man: false,
        };
        let err = run(&cli).await.unwrap_err();
        assert!(err.to_string().contains("Failed to load config"));
    }

    #[tokio::test]
    async fn run_fails_when_authentication_is_rejected() {
        let server = MockServer::start(vec![MockResponse::json(401, "{}")]);
        let dir = tempfile::tempdir().unwrap();
        let cli = setup_workspace(dir.path(), &server.url);

        let err = run(&cli).await.unwrap_err();
        assert!(err.to_string().contains("Authentication check failed"));
    }
}
