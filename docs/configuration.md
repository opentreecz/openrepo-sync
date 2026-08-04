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
| `download_dir` | No | system temp | Directory for temporary package downloads |

### API Key

The API key is available from your OpenRepo user profile (via `GET /api/whoami` → `api_key` field). Store it as an environment variable and reference it with `${OPENREPO_API_KEY}` to keep secrets out of config files.

---

## Per-Project Files (`projects/<name>.yaml`)

Every project file requires `name`, `repo_uid`, `keep_versions`, and a `source` block; `on_conflict` is optional:

| Field | Type | Required | Description |
|---|---|---|---|
| `name` | string | Yes | Identifier used in log output and with `--project` |
| `repo_uid` | string | Yes | Target OpenRepo repository identifier |
| `keep_versions` | integer | Yes | Number of versions to retain; older ones are deleted |
| `on_conflict` | string | No | What to do when the uploaded package already exists in the repository: `error` (default), `skip`, or `overwrite` |
| `source` | object | Yes | Upstream source configuration (see [Source Types](../sources/)) |

### `on_conflict`

| Value | Behaviour when OpenRepo rejects an upload as a duplicate |
|---|---|
| `error` | The project fails and the process exits `1` (default) |
| `skip` | The package is logged as skipped and the sync continues |
| `overwrite` | The upload is sent with the overwrite flag, replacing the existing package |

### Example

```yaml
name: curl
repo_uid: debian-stable
keep_versions: 3
on_conflict: skip
source:
  type: github
  owner: curl
  repo: curl
  asset_filter: "*.deb"
```

---

## Environment Variables

| Variable | Description |
|---|---|
| `OPENREPO_API_KEY` | Expanded when referenced as `${OPENREPO_API_KEY}` in `config.yaml` |
| `RUST_LOG` | Log filter — overrides `--verbose`. E.g. `RUST_LOG=openrepo=debug,reqwest=warn` |
