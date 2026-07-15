mod config;
mod models;
mod repo_client;
mod sources;
mod sync;
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

    if had_error {
        std::process::exit(1);
    }

    Ok(())
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
    let cmd = Cli::command();
    let man = clap_mangen::Man::new(cmd);
    let mut buf = Vec::new();
    man.render(&mut buf)?;
    print!("{}", String::from_utf8(buf)?);
    Ok(())
}
