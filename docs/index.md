---
layout: home
title: openrepo-sync
---

<p align="center">
  <a href="https://github.com/opentreecz">
    <img src="assets/logo.png" alt="OpenTree" width="128" height="128">
  </a>
</p>

# openrepo-sync

A command-line tool that keeps a self-hosted [OpenRepo](https://github.com/opentreecz/openrepo) package repository in sync with upstream software sources.

> **Server:** openrepo-sync requires a running [OpenRepo](https://github.com/opentreecz/openrepo) instance ([documentation](https://opentreecz.github.io/openrepo/)) as the target package repository. OpenRepo hosts .deb, .rpm, and generic packages with APT/YUM metadata generation, PGP signing, and a web management interface.

**openrepo-sync** fetches new package versions from GitHub Releases, external Debian APT repositories, RPM (YUM/DNF) repositories, direct download URLs, and SourceForge, uploads them to OpenRepo, and prunes releases older than a configured threshold.

---

## Features

- **6 upstream source types** — GitHub Releases, Debian APT repositories, RPM (YUM/DNF) repositories, static URLs, LATEST URLs, SourceForge
- **Architecture-aware GitHub downloads** — `arch_filter` selects the correct asset when a release publishes multiple architecture variants; `amd64`/`x86_64`/`x86-64` and `arm64`/`aarch64` are treated as aliases
- **Debian APT repository mirroring** — fetches `Packages.gz`/`Packages` index, filters by package name and/or filename glob, supports multiple suites/components/architectures, optional GPG signature verification
- **Automatic version detection** — extracts versions from filenames, or calls `dpkg-deb`/`rpm` on the package itself for LATEST URLs
- **Configurable retention** — keep the N newest releases, auto-prune the rest
- **Dry-run mode** — preview all actions without touching the repository
- **Per-project YAML files** — one file per tracked package; easy to add, remove, or disable
- **`${ENV_VAR}` expansion** in config values for safe API key handling
- **Structured logging** — quiet by default, full debug via `--verbose` or `RUST_LOG`
- **Multi-platform Docker image** — `linux/amd64`, `linux/arm64`, `linux/arm/v7`

---

## Quick Start

```sh
# 1. Edit the global config
cp config.yaml.example config.yaml
$EDITOR config.yaml

# 2. Add a project (pick the template for your source type)
mkdir -p projects/
cp projects/github-example.yaml.example   projects/curl.yaml
cp projects/deb-repo-example.yaml.example projects/nginx.yaml
$EDITOR projects/curl.yaml

# 3. Dry run — preview without writing anything
openrepo-sync --dry-run

# 4. Sync
openrepo-sync
```

---

## Source Types at a Glance

| Type | Description |
|---|---|
| [`github`](sources/#github) | GitHub Releases API — picks correct arch asset automatically |
| [`deb_repo`](sources/#deb_repo) | Debian APT repository, including OBS flat layout (Packages.gz index) |
| [`rpm_repo`](sources/#rpm_repo) | RPM (YUM/DNF) repository (repomd.xml + primary.xml/sqlite) |
| [`direct_url`](sources/#direct_url) | Fixed URL with version in the filename |
| [`direct_url_latest`](sources/#direct_url_latest) | Fixed URL, version extracted from package metadata |
| [`sourceforge`](sources/#sourceforge) | SourceForge file releases |

---

## Navigation

| Page | Description |
|---|---|
| [Installation](install/) | Binary packages, build from source, Docker |
| [Configuration](configuration/) | Global config and per-project YAML schema |
| [Source Types](sources/) | All 5 source types with full field reference and examples |
| [Usage](usage/) | CLI reference, examples, logging and debugging |
| [Docker](docker/) | Multi-platform container — setup, scheduling, troubleshooting |
| [API Reference](api/) | OpenRepo REST API endpoints used by this tool |
| [Coverage](coverage/) | Test coverage report and CI integration |
