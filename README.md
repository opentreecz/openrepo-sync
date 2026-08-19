# openrepo-sync

[![CI](https://github.com/opentreecz/openrepo-sync/actions/workflows/ci.yml/badge.svg)](https://github.com/opentreecz/openrepo-sync/actions/workflows/ci.yml)
[![Release](https://github.com/opentreecz/openrepo-sync/actions/workflows/release.yml/badge.svg)](https://github.com/opentreecz/openrepo-sync/actions/workflows/release.yml)
[![codecov](https://codecov.io/gh/opentreecz/openrepo-sync/branch/main/graph/badge.svg)](https://codecov.io/gh/opentreecz/openrepo-sync)
[![Coverage](https://img.shields.io/badge/coverage-90%25-brightgreen)](https://opentreecz.github.io/openrepo-sync/coverage/)
[![Docker](https://img.shields.io/badge/docker-ghcr.io-blue)](https://github.com/opentreecz/openrepo-sync/pkgs/container/openrepo-sync)

A command-line tool that keeps a self-hosted [OpenRepo](https://github.com/opentreecz/openrepo) package repository in sync with upstream software sources. It fetches new package versions from GitHub Releases, external Debian (APT) repositories, direct download URLs, and SourceForge, uploads them to OpenRepo, and prunes releases older than a configured threshold.

**[Full documentation → opentreecz.github.io/openrepo-sync](https://opentreecz.github.io/openrepo-sync/)**

---

## Features

- **5 upstream source types** — GitHub Releases, Debian APT repositories, static URLs, LATEST URLs, SourceForge
- **Architecture-aware GitHub downloads** — `arch_filter` picks the right asset (amd64/x86_64/arm64/aarch64 aliases) when a release ships multiple architecture variants
- **Debian APT repository mirroring** — fetches `Packages.gz`/`Packages`, filters by package name and/or filename glob, supports multiple suites/components/architectures, optional GPG signature verification
- **Automatic version detection** — extracts versions from filenames, or calls `dpkg-deb`/`rpm` on the package itself for LATEST URLs
- **Configurable retention** — keep the N newest releases, auto-prune the rest
- **Dry-run mode** — preview all actions without modifying the repository
- **Per-project YAML config files** — one file per tracked package; easy to add, remove, or disable
- **`${ENV_VAR}` expansion** in config values for safe API key handling
- **Structured logging** — quiet by default, full debug via `--verbose` or `RUST_LOG`
- **Multi-platform Docker image** — `linux/amd64`, `linux/arm64`, `linux/arm/v7`

---

## Installation

### From a release package

Download the `.deb` or `.rpm` for your platform from the [latest release](https://github.com/opentreecz/openrepo-sync/releases/latest):

```sh
# Debian / Ubuntu
sudo dpkg -i openrepo-sync_*_amd64.deb

# RHEL / Fedora
sudo rpm -i openrepo-sync-*-1.x86_64.rpm
```

### Docker

```sh
docker pull ghcr.io/opentreecz/openrepo-sync:latest
```

### From source

```sh
git clone https://github.com/opentreecz/openrepo-sync
cd openrepo-sync
cargo build --release
install -m755 target/release/openrepo-sync /usr/local/bin/
```

---

## Quick Start

**1. Create the global config:**

```sh
cp config.yaml.example config.yaml
$EDITOR config.yaml   # set api_url and api_key
```

**2. Create a project file for each package to track:**

```sh
mkdir -p projects/
# Copy the template for your source type:
cp projects/github-example.yaml.example        projects/curl.yaml
cp projects/deb-repo-example.yaml.example      projects/nginx.yaml
cp projects/sourceforge-example.yaml.example   projects/sfpkg.yaml
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

---

## Configuration

### Global config (`config.yaml`)

```yaml
openrepo:
  api_url: "https://openrepo.example.com"
  api_key: "${OPENREPO_API_KEY}"   # ${VAR} expanded from the environment
download_dir: "/tmp/openrepo-sync" # optional; defaults to the system temp dir
```

### Per-project files (`projects/<name>.yaml`)

| Field | Description |
|---|---|
| `name` | Identifier used in log output and `--project` |
| `repo_uid` | Target OpenRepo repository UID |
| `keep_versions` | Maximum number of versions to retain (older ones are deleted) |
| `on_conflict` | What to do if the package already exists: `error` (default), `skip`, `overwrite` |
| `source` | Upstream source configuration (see Source Types below) |

---

## Source Types

### `github` — GitHub Releases

```yaml
source:
  type: github
  owner: curl               # GitHub org or user
  repo: curl                # repository name
  asset_filter: "*.deb"    # optional glob; omit to keep all assets
  prerelease: false         # default: false

  # Architecture preference when a release has multiple arch assets.
  # First match wins. amd64/x86_64/x86-64 and arm64/aarch64 are treated
  # as aliases. Default: [amd64, arm64]. Set [] to disable.
  arch_filter: [amd64, arm64]
```

**Defaults:** `prerelease: false`, `arch_filter: [amd64, arm64]`.

### `deb_repo` — Debian APT Repository

Mirrors packages directly from any standard Debian repository (Packages.gz / Packages index).

```yaml
source:
  type: deb_repo
  url: https://nginx.org/packages/debian   # repository base URL
  suites: bookworm                         # single string or list
  components: nginx                        # single string or list
  architectures: [amd64, arm64]            # single string or list

  package_filter: nginx                    # exact Package: field match (optional)
  filename_filter: "nginx_*.deb"           # glob on filename (optional)

  verify_gpg: true                         # verify InRelease signature (default: true)
  gpg_key: https://nginx.org/keys/nginx_signing.key  # URL or inline ASCII-armored key
```

**Defaults:** `suites: [bookworm]`, `components: [main]`, `architectures: [amd64]`, `verify_gpg: true`. Set `verify_gpg: false` to disable GPG signature verification.

### `direct_url` — Static URL

```yaml
source:
  type: direct_url
  url: "https://example.com/releases/mypkg-2.1.0.deb"
```

Version is extracted from the filename by regex.

### `direct_url_latest` — LATEST URL (version in package metadata)

```yaml
source:
  type: direct_url_latest
  url: "https://example.com/releases/mypkg-LATEST.deb"
```

Downloads the file, extracts version via `dpkg-deb` (`.deb`) or `rpm -qp` (`.rpm`), renames it, and uploads. Requires `dpkg` or `rpm` installed on the host.

### `sourceforge` — SourceForge File Releases

```yaml
source:
  type: sourceforge
  project: my-sf-project
  folder: "releases/linux"   # optional subfolder
  filename_filter: "*.deb"   # optional glob
```

All fields except `project` are optional. Defaults: root listing (no `folder`), all files (no `filename_filter`).

---

## Docker

```sh
cp config.yaml.example config.yaml && $EDITOR config.yaml
cp projects/github-example.yaml.example projects/curl.yaml && $EDITOR projects/curl.yaml
echo "OPENREPO_API_KEY=your_token_here" > .env

docker compose run --rm openrepo-sync --dry-run   # preview
docker compose run --rm openrepo-sync             # run for real
```

### Build from source

To build the image locally from source (no Rust toolchain required):

```sh
docker compose build
docker compose run --rm openrepo-sync --dry-run
```

See the [Docker documentation](https://opentreecz.github.io/openrepo-sync/docker/#building-locally-from-source) for details.

See the [Docker documentation](https://opentreecz.github.io/openrepo-sync/docker/) for the full walkthrough including scheduling with cron/systemd.

---

## Usage

```
openrepo-sync [OPTIONS]

Options:
  --config <FILE>     Global config file         [default: config.yaml]
  --projects <DIR>    Per-project YAML directory [default: projects/]
  --project <NAME>    Sync only the named project
  --dry-run           Preview actions without uploading or deleting
  -v, --verbose       Enable debug logging
  -h, --help          Show help
  -V, --version       Show version
```

---

## Requirements

| Requirement | When needed |
|---|---|
| A running [OpenRepo](https://github.com/opentreecz/openrepo) instance | Always |
| `dpkg-deb` (package: `dpkg`) | `direct_url_latest` with `.deb` packages |
| `rpm` | `direct_url_latest` with `.rpm` packages |
| `gpg` | `deb_repo` with `verify_gpg: true` |

---

## Project Structure

```
src/
├── main.rs              CLI entry point
├── config.rs            YAML config loading, ${ENV_VAR} expansion
├── models.rs            PackageVersion, RemotePackage, SyncResult
├── version.rs           Version extraction from filenames, dpkg-deb, rpm
├── repo_client.rs       OpenRepo REST API client
├── sync.rs              Per-project sync orchestration
└── sources/
    ├── deb_repo.rs      Debian APT repository (Packages index)
    ├── direct_url.rs    Static URL and LATEST URL sources
    ├── github.rs        GitHub Releases API
    └── sourceforge.rs   SourceForge file listing scraper
projects/
├── deb-repo-example.yaml.example
├── direct-url-example.yaml.example
├── direct-url-latest-example.yaml.example
├── github-example.yaml.example
└── sourceforge-example.yaml.example
```

---

## Documentation

Full documentation is available at **[opentreecz.github.io/openrepo-sync](https://opentreecz.github.io/openrepo-sync/)**:

- [Installation](https://opentreecz.github.io/openrepo-sync/install/)
- [Configuration](https://opentreecz.github.io/openrepo-sync/configuration/)
- [Source Types](https://opentreecz.github.io/openrepo-sync/sources/)
- [Usage & CLI reference](https://opentreecz.github.io/openrepo-sync/usage/)
- [Docker](https://opentreecz.github.io/openrepo-sync/docker/)
- [API Reference](https://opentreecz.github.io/openrepo-sync/api/)
- [Test Coverage](https://opentreecz.github.io/openrepo-sync/coverage/)

---

## License

See [LICENSE](LICENSE).
