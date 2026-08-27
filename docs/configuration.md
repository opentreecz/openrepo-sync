---
layout: page
title: Configuration
permalink: /configuration/
---

<p align="center">
  <a href="https://github.com/opentreecz">
    <img src="assets/logo.png" alt="OpenTree" width="96" height="96">
  </a>
</p>

# Configuration

Configuration is split into two layers:

| File | Purpose |
|---|---|
| `config.yaml` | Global settings: OpenRepo server URL, API key, download directory, schedule |
| `projects/*.yaml` | One file per tracked software package |

---

## Global Config (`config.yaml`)

> The `api_url` and `api_key` connect to your [OpenRepo](https://github.com/opentreecz/openrepo) server instance. See the [OpenRepo getting started guide](https://opentreecz.github.io/openrepo/getting-started/) for server setup instructions.

```yaml
openrepo:
  api_url: "https://openrepo.example.com"
  api_key: "${OPENREPO_API_KEY}"   # ${VAR} is expanded from the environment
download_dir: "/tmp/openrepo-sync" # optional; defaults to the system temp dir
schedule:
  enabled: true
  interval: "24h"
  run_on_start: true
```

### Fields

| Field | Required | Default | Description |
|---|---|---|---|
| `openrepo.api_url` | Yes | — | Base URL of your OpenRepo instance |
| `openrepo.api_key` | Yes | — | API token. Supports `${ENV_VAR}` expansion |
| `download_dir` | No | system temp dir | Directory for temporary package downloads |

### Schedule

The `schedule` block is used when the CLI is started with `--schedule`, including the default `docker compose up -d` setup. Normal one-shot commands ignore it.

| Field | Required | Default | Description |
|---|---|---|---|
| `schedule.enabled` | No | `true` | Enables scheduled sync passes while running with `--schedule` |
| `schedule.interval` | No | `24h` | Delay between sync passes. Supports `m`, `h`, and `d`, such as `30m`, `6h`, `24h`, or `1d` |
| `schedule.run_on_start` | No | `true` | Run the first sync immediately when the scheduler starts |

Example: run every six hours, starting immediately:

```yaml
schedule:
  enabled: true
  interval: "6h"
  run_on_start: true
```

### API Key

The API key is available from your OpenRepo user profile (or `GET /api/whoami` → `api_key` field). Store it as an environment variable and reference it as `${OPENREPO_API_KEY}` to keep secrets out of config files and version control.

---

## Per-Project Files (`projects/<name>.yaml`)

Every project file requires these top-level fields:

| Field | Type | Required | Default | Description |
|---|---|---|---|---|
| `name` | string | Yes | — | Identifier used in log output and with `--project` |
| `repo_uid` | string | Yes | — | Target OpenRepo repository identifier |
| `keep_versions` | integer | Yes | — | Number of versions to retain per package name and architecture; older ones are deleted |
| `on_conflict` | string | No | `error` | `error`, `skip`, or `overwrite` |
| `source` | object | Yes | — | Upstream source configuration (see [Source Types](../sources/)) |

### `on_conflict`

| Value | Behaviour |
|---|---|
| `error` | Return an error if the package already exists **(default)** |
| `skip` | Silently skip the upload if the package already exists |
| `overwrite` | Replace the existing package |

---

## Source Type Quick Reference

See [Source Types](../sources/) for the full field reference and examples.

### `github`
```yaml
source:
  type: github
  owner: curl
  repo: curl
  asset_filter: "*.deb"              # optional; omit to keep all assets
  prerelease: false                   # default: false
  arch_filter: [amd64, arm64]         # default: [amd64, arm64]
```

### `deb_repo`
```yaml
source:
  type: deb_repo
  url: https://nginx.org/packages/debian
  layout: debian                            # default: debian; use flat for OBS-style repos
  suites: bookworm                            # default: [bookworm]
  components: nginx                           # default: [main]
  architectures: [amd64, arm64]               # default: [amd64]
  package_filter: nginx                       # optional; string or list
  # package_filter: [nginx, nginx-module-njs]
  verify_gpg: true                            # default: true; set to false to skip GPG verification
  gpg_key: https://nginx.org/keys/nginx_signing.key
```

Flat OBS-style APT repositories place `Packages` at the repository root:

```yaml
source:
  type: deb_repo
  layout: flat
  url: https://download.opensuse.org/repositories/home:/CZ-NIC:/datovka-latest/Debian_13
  package_filter: datovka
  filename_filter: "datovka_*_amd64.deb"
```

`keep_versions` is enforced separately for each `(package_name, architecture)` group. If `package_filter` lists multiple packages, each package/architecture group keeps its own newest N versions.

### `rpm_repo`
```yaml
source:
  type: rpm_repo
  url: https://download.fedoraproject.org/pub/epel/9/Everything/x86_64
  architectures: [x86_64, noarch]         # default: [x86_64, noarch]
  package_filter: nginx                   # optional; exact name match
  verify_gpg: true                        # default: true
  gpg_key: https://www.redhat.com/security/team/key/
```

### `direct_url`
```yaml
source:
  type: direct_url
  url: "https://example.com/mypkg-2.1.0.deb"
```

### `direct_url_latest`
```yaml
source:
  type: direct_url_latest
  url: "https://example.com/mypkg-LATEST.deb"
```

### `sourceforge`
```yaml
source:
  type: sourceforge
  project: my-sf-project
  folder: "releases/linux"            # optional; default: root listing
  filename_filter: "*.deb"            # optional; default: all files
```

---

## Environment Variables

| Variable | Description |
|---|---|
| `OPENREPO_API_KEY` | Expanded when referenced as `${OPENREPO_API_KEY}` in `config.yaml` |
| `RUST_LOG` | Log filter — overrides `--verbose`. Example: `RUST_LOG=openrepo=debug,reqwest=warn` |

Any `${VAR}` pattern in a config file is expanded from the process environment. If the variable is not set, the literal `${VAR}` string is kept as-is.

---

## Project Templates

The `projects/` directory in the repository contains one ready-to-copy template per source type:

| Template file | Source type |
|---|---|
| `github-example.yaml.example` | `github` |
| `deb-repo-example.yaml.example` | `deb_repo` |
| `direct-url-example.yaml.example` | `direct_url` |
| `direct-url-latest-example.yaml.example` | `direct_url_latest` |
| `sourceforge-example.yaml.example` | `sourceforge` |

Copy the relevant template and remove the `.example` suffix to activate it:

```sh
cp projects/deb-repo-example.yaml.example projects/nginx.yaml
$EDITOR projects/nginx.yaml
```
