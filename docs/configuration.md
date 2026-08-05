---
layout: page
title: Configuration
permalink: /configuration/
---

# Configuration

Configuration is split into two layers:

| File | Purpose |
|---|---|
| `config.yaml` | Global settings: OpenRepo server URL, API key, download directory |
| `projects/*.yaml` | One file per tracked software package |

---

## Global Config (`config.yaml`)

```yaml
openrepo:
  api_url: "https://openrepo.example.com"
  api_key: "${OPENREPO_API_KEY}"   # ${VAR} is expanded from the environment
download_dir: "/tmp/openrepo-sync" # optional; defaults to the system temp dir
```

### Fields

| Field | Required | Default | Description |
|---|---|---|---|
| `openrepo.api_url` | Yes | — | Base URL of your OpenRepo instance |
| `openrepo.api_key` | Yes | — | API token. Supports `${ENV_VAR}` expansion |
| `download_dir` | No | system temp dir | Directory for temporary package downloads |

### API Key

The API key is available from your OpenRepo user profile (or `GET /api/whoami` → `api_key` field). Store it as an environment variable and reference it as `${OPENREPO_API_KEY}` to keep secrets out of config files and version control.

---

## Per-Project Files (`projects/<name>.yaml`)

Every project file requires these top-level fields:

| Field | Type | Required | Description |
|---|---|---|---|
| `name` | string | Yes | Identifier used in log output and with `--project` |
| `repo_uid` | string | Yes | Target OpenRepo repository identifier |
| `keep_versions` | integer | Yes | Number of versions to retain; older ones are deleted |
| `on_conflict` | string | No | `error` (default), `skip`, or `overwrite` |
| `source` | object | Yes | Upstream source configuration (see [Source Types](../sources/)) |

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
  asset_filter: "*.deb"
  arch_filter: [amd64, arm64]   # default: prefer amd64
```

### `deb_repo`
```yaml
source:
  type: deb_repo
  url: https://nginx.org/packages/debian
  suites: bookworm
  components: nginx
  architectures: [amd64, arm64]
  package_filter: nginx
  verify_gpg: true
  gpg_key: https://nginx.org/keys/nginx_signing.key
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
  folder: "releases/linux"
  filename_filter: "*.deb"
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
