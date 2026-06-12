---
layout: home
title: openrepo-sync
---

# openrepo-sync

A command-line tool that keeps a self-hosted [OpenRepo](https://github.com/openkilt/openrepo) package repository in sync with upstream software sources.

**openrepo-sync** checks GitHub Releases, direct download URLs, and SourceForge for new package versions, uploads them to OpenRepo, and removes releases older than a configured threshold.

---

## Features

- **Multiple upstream sources** — GitHub Releases, direct URLs (static or LATEST), SourceForge
- **Automatic version detection** — extracts versions from filenames, or calls `dpkg-deb`/`rpm` on the package itself
- **Configurable retention** — keep the N newest releases, auto-prune the rest
- **Dry-run mode** — preview all actions without touching the repository
- **Per-project YAML files** — easy to add, remove, or disable individual packages
- **`${ENV_VAR}` expansion** in config values for safe API key handling
- **Structured logging** — quiet by default, full debug via `--verbose` or `RUST_LOG`
- **Multi-platform Docker image** — `linux/amd64`, `linux/arm64`, `linux/arm/v7`

---

## Quick Start

```sh
# 1. Edit the global config
cp config.yaml.example config.yaml
$EDITOR config.yaml

# 2. Add a project
mkdir -p projects/
cat > projects/curl.yaml <<EOF
name: curl
repo_uid: debian-stable
keep_versions: 3
source:
  type: github
  owner: curl
  repo: curl
  asset_filter: "*.deb"
EOF

# 3. Dry run
openrepo-sync --dry-run

# 4. Sync
openrepo-sync
```

---

## Navigation

| Page | Description |
|---|---|
| [Installation](install/) | Build from source, install binary and man page |
| [Configuration](configuration/) | Global config and per-project YAML schema |
| [Usage](usage/) | CLI reference, examples, logging and debugging |
| [Source Types](sources/) | GitHub, direct URL, LATEST URL, SourceForge |
| [API Reference](api/) | OpenRepo REST API endpoints used by this tool |
| [Docker](docker/) | Multi-platform container image |
| [Coverage](coverage/) | Test coverage report and CI integration |
