# openrepo-sync

[![CI](https://github.com/opentreecz/openrepo-sync/actions/workflows/ci.yml/badge.svg)](https://github.com/opentreecz/openrepo-sync/actions/workflows/ci.yml)
[![codecov](https://codecov.io/gh/opentreecz/openrepo-sync/branch/main/graph/badge.svg)](https://codecov.io/gh/opentreecz/openrepo-sync)

A command-line tool that keeps a self-hosted [OpenRepo](https://github.com/openkilt/openrepo) package repository in sync with upstream software sources. It checks GitHub Releases, direct download URLs, and SourceForge for new package versions, uploads them to OpenRepo, and removes releases older than a configured threshold.

## Features

- Tracks multiple upstream sources per project: GitHub Releases, direct URLs, SourceForge
- Supports packages where the download URL contains no version — uses `dpkg-deb` or `rpm` to extract the real version from the package metadata
- Configurable retention: keep the N newest releases, auto-prune the rest
- Dry-run mode: preview all actions without modifying the repository
- Per-project YAML config files — easy to add, remove, or disable individual packages
- `${ENV_VAR}` expansion in config values for safe API key handling
- Structured logging via `RUST_LOG` for fine-grained debug output

## Requirements

- Rust 1.70+
- A running [OpenRepo](https://github.com/openkilt/openrepo) instance
- `dpkg-deb` (package: `dpkg`) — required only for `direct_url_latest` with `.deb` packages
- `rpm` — required only for `direct_url_latest` with `.rpm` packages

## Installation

```sh
cargo build --release
install -m755 target/release/openrepo-sync /usr/local/bin/

# Optional: install man page
install -Dm644 man/man1/openrepo-sync.1 /usr/local/share/man/man1/openrepo-sync.1
mandb
```

## Quick Start

**1. Create the global config:**

```sh
cp config.yaml.example config.yaml
$EDITOR config.yaml
```

**2. Create a project file for each package to track:**

```sh
mkdir -p projects/
cp projects/github-example.yaml.example projects/curl.yaml   # or another *-example.yaml.example template
$EDITOR projects/curl.yaml
```

**3. Preview what would happen:**

```sh
openrepo-sync --dry-run
```

**4. Run for real:**

```sh
openrepo-sync
```

## Docker

A default `config.yaml.example` and one `projects/*-example.yaml.example` template per source type are included in this repo, ready to be copied and filled in.

```sh
cp config.yaml.example config.yaml && $EDITOR config.yaml
cp projects/github-example.yaml.example projects/curl.yaml && $EDITOR projects/curl.yaml
echo "OPENREPO_API_KEY=your_token_here" > .env

docker compose run --rm openrepo-sync --dry-run   # preview
docker compose run --rm openrepo-sync             # run for real
```

See [docs/docker.md](docs/docker.md) for the full step-by-step walkthrough, including cron/systemd scheduling and troubleshooting.

## Configuration

Configuration is split into two layers:

| File | Purpose |
|---|---|
| `config.yaml` | Global settings: OpenRepo server URL, API key, download directory |
| `projects/*.yaml` | One file per tracked software package |

### Global config (`config.yaml`)

```yaml
openrepo:
  api_url: "https://openrepo.example.com"
  api_key: "${OPENREPO_API_KEY}"   # ${VAR} is expanded from the environment
download_dir: "/tmp/openrepo-sync" # optional; defaults to the system temp dir
```

The API key is available from the OpenRepo web UI under your user profile. Store it in the environment and reference it as `${OPENREPO_API_KEY}` to keep it out of version control.

### Per-project files (`projects/<name>.yaml`)

Every project file requires four fields:

| Field | Description |
|---|---|
| `name` | Identifier used in log output and with `--project` |
| `repo_uid` | Target OpenRepo repository identifier |
| `keep_versions` | Maximum number of versions to retain; older ones are deleted |
| `source` | Upstream source configuration (see Source Types) |

## Source Types

### `github` — GitHub Releases

Fetches release assets via the GitHub Releases API.

```yaml
name: curl
repo_uid: debian-stable
keep_versions: 3
source:
  type: github
  owner: curl
  repo: curl
  asset_filter: "*.deb"   # optional glob; omit to keep all assets per release
  prerelease: false        # optional, default: false
```

- Version is taken from the release `tag_name` (e.g. `v8.5.0` → `8.5.0`)
- Draft releases are always skipped
- Unauthenticated requests are subject to GitHub's 60 req/hour rate limit per IP

### `direct_url` — Static URL

A fixed URL where the filename already contains the version string.

```yaml
name: mypkg
repo_uid: debian-custom
keep_versions: 1
source:
  type: direct_url
  url: "https://example.com/releases/mypkg-2.1.0.deb"
```

Version is extracted from the filename by regex, matching patterns such as `name-1.2.3.deb`, `name_1.2.3_amd64.deb`, `name-v1.2.3.tar.gz`.

### `direct_url_latest` — URL with no version in the filename

For sources that publish a file at a fixed URL (e.g. `mypkg-LATEST.deb`). The file is downloaded first; then `dpkg-deb` (`.deb`) or `rpm -qp` (`.rpm`) is called to read the version from the package metadata. The file is renamed to include the version before upload.

```yaml
name: mypkg-latest
repo_uid: debian-custom
keep_versions: 1
source:
  type: direct_url_latest
  url: "https://example.com/releases/mypkg-LATEST.deb"
```

Requires `dpkg-deb` or `rpm` to be installed on the host.

### `sourceforge` — SourceForge

Scrapes the SourceForge file listing page to discover releases.

```yaml
name: sfpkg
repo_uid: debian-sf
keep_versions: 2
source:
  type: sourceforge
  project: my-sf-project
  folder: "releases/linux"   # optional subfolder; omit for root listing
  filename_filter: "*.deb"   # optional glob filter
```

Files are sorted by detected version number, newest first.

## Usage

```
openrepo-sync [OPTIONS]

Options:
  --config <FILE>     Global config file             [default: config.yaml]
  --projects <DIR>    Per-project YAML directory     [default: projects/]
  --project <NAME>    Sync only the named project
  --dry-run           Preview actions without uploading or deleting
  -v, --verbose       Enable debug logging
  -h, --help          Show help
  -V, --version       Show version
```

### Examples

```sh
# Sync all projects
openrepo-sync

# Sync a single project
openrepo-sync --project curl

# Preview what would change without touching the repository
openrepo-sync --dry-run

# Verbose output for a single project
openrepo-sync --project curl --verbose

# Non-default config paths
openrepo-sync \
  --config /etc/openrepo-sync/config.yaml \
  --projects /etc/openrepo-sync/projects

# Cron job: show only warnings and errors
RUST_LOG=warn openrepo-sync --config /etc/openrepo-sync/config.yaml
```

## Logging and Debugging

`openrepo-sync` uses structured logging via the [`tracing`](https://docs.rs/tracing) crate.

| Mode | What is shown |
|---|---|
| Default (info) | Auth result, per-project status, errors |
| `--verbose` / `-v` | + config paths, URL fetches, package counts, API requests, module paths |
| `RUST_LOG=debug` | Same as `--verbose`; `RUST_LOG` takes precedence when set |

Fine-grained filtering with `RUST_LOG`:

```sh
# All debug output
RUST_LOG=debug openrepo-sync

# Own code at debug, suppress noisy HTTP internals
RUST_LOG=openrepo=debug,reqwest=warn,hyper=warn openrepo-sync

# Trace a single module
RUST_LOG=openrepo_sync::sync=trace openrepo-sync

# Pipe verbose output through a pager
RUST_LOG=debug openrepo-sync --project curl 2>&1 | less
```

## Sync Workflow

For each project, in order:

1. Verify authentication via `GET /api/whoami` (once per run, on startup)
2. Fetch the latest `keep_versions` releases from the upstream source
3. List packages currently in the OpenRepo repository
4. Diff by filename — identify remote packages not yet present in the repo
5. For each missing package: download → upload → remove local temp file
6. Re-fetch the repo package list and sort by version descending
7. Delete packages beyond `keep_versions`, keeping only the newest

If a project fails, the error is printed to stderr and remaining projects continue. The process exits with code 1 if any project had an error.

## Exit Codes

| Code | Meaning |
|---|---|
| `0` | All projects synced successfully |
| `1` | One or more projects encountered an error |

## Project Structure

```
src/
├── main.rs              CLI entry point (clap)
├── config.rs            YAML config loading, ${ENV_VAR} expansion
├── models.rs            PackageVersion, RemotePackage, RepoPackage, SyncResult
├── version.rs           Version extraction from filenames, dpkg-deb, rpm
├── repo_client.rs       OpenRepo REST API client
├── sync.rs              Per-project sync orchestration
└── sources/
    ├── mod.rs           PackageSource trait
    ├── github.rs        GitHub Releases API
    ├── direct_url.rs    Static URL and LATEST URL sources
    └── sourceforge.rs   SourceForge file listing scraper
man/
└── man1/
    └── openrepo-sync.1  Man page
config.yaml.example      Global config template — copy to config.yaml
projects/
└── *-example.yaml.example  One template per source type — copy to projects/<name>.yaml
docker-compose.yml        Default Docker Compose service definition
USAGE.txt                Plain text reference documentation
```

## License

See [LICENSE](LICENSE).
