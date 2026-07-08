---
layout: page
title: Docker
permalink: /docker/
---

# Docker

`openrepo-sync` is published as a multi-platform Docker image to the GitHub Container Registry (GHCR). This page is a complete, step-by-step walkthrough for running it in a container, using the default config templates shipped in the repository.

## Supported Platforms

| Docker platform | Architecture |
|---|---|
| `linux/amd64` | x86-64 |
| `linux/arm64` | ARM 64-bit |
| `linux/arm/v7` | ARM 32-bit hard-float |

---

## Step-by-Step Setup

### 1. Create a working directory

This directory holds your real config and is bind-mounted into the container. It will contain your OpenRepo API key (indirectly, via `.env`), so keep it outside of any public git repo.

```sh
mkdir -p /etc/openrepo-sync/projects
cd /etc/openrepo-sync
```

### 2. Fetch the default config templates

If you cloned the source repo, the templates are already at its root (`config.yaml.example`, `projects/*.yaml.example`, `docker-compose.yml`). Otherwise, download them directly:

```sh
curl -fsSL -o config.yaml.example \
  https://raw.githubusercontent.com/opentreecz/openrepo-sync/main/config.yaml.example
curl -fsSL -o docker-compose.yml \
  https://raw.githubusercontent.com/opentreecz/openrepo-sync/main/docker-compose.yml
```

### 3. Create the global config

```sh
cp config.yaml.example config.yaml
$EDITOR config.yaml
```

Set `openrepo.api_url` to your OpenRepo server's base URL. Leave `api_key` as `${OPENREPO_API_KEY}` — the real value is supplied via environment variable in the next step, not written into the file.

```yaml
openrepo:
  api_url: "https://openrepo.example.com"
  api_key: "${OPENREPO_API_KEY}"
```

### 4. Store the API key outside the config file

Get the key from the OpenRepo web UI (user profile page), then:

```sh
echo "OPENREPO_API_KEY=your_token_here" > .env
chmod 600 .env
```

`docker compose` reads `.env` automatically. **Never commit this file.**

### 5. Add at least one project file

Pick the template matching your upstream source and copy it into `projects/`. See [Source Types](../sources/) for the full field reference on each one.

```sh
# GitHub Releases
curl -fsSL -o projects/curl.yaml \
  https://raw.githubusercontent.com/opentreecz/openrepo-sync/main/projects/github-example.yaml.example

$EDITOR projects/curl.yaml
```

Repeat for as many packages as you want to track — one YAML file per package. Available templates:

| Template | Source type | Use when… |
|---|---|---|
| `github-example.yaml.example` | `github` | Upstream publishes `.deb`/`.rpm` on GitHub Releases |
| `direct-url-example.yaml.example` | `direct_url` | Fixed URL, filename already contains the version |
| `direct-url-latest-example.yaml.example` | `direct_url_latest` | Fixed "LATEST" URL, version only in package metadata |
| `sourceforge-example.yaml.example` | `sourceforge` | Upstream publishes via SourceForge file releases |

### 6. Pull the image

```sh
docker pull ghcr.io/opentreecz/openrepo-sync:latest
```

### 7. Dry run — verify before touching anything

Always dry-run first. It authenticates against OpenRepo and prints every action it *would* take, without uploading or deleting anything.

```sh
docker run --rm \
  --env-file .env \
  -v ./config.yaml:/config.yaml:ro \
  -v ./projects:/projects:ro \
  ghcr.io/opentreecz/openrepo-sync:latest \
  --config /config.yaml --projects /projects --dry-run --verbose
```

Check the output for:
- `Authenticated as: <your username>` — confirms `api_url`/`api_key` are correct
- One line per project showing what it would upload or prune

If authentication fails, re-check `api_url` and that `OPENREPO_API_KEY` in `.env` matches the key from the OpenRepo UI.

### 8. Run for real

Once the dry run looks right, drop `--dry-run`:

```sh
docker run --rm \
  --env-file .env \
  -v ./config.yaml:/config.yaml:ro \
  -v ./projects:/projects:ro \
  ghcr.io/opentreecz/openrepo-sync:latest \
  --config /config.yaml --projects /projects
```

The container exits `0` on success, `1` if any project errored (see the process output for details).

### 9. Switch to Docker Compose (recommended for repeated runs)

The `docker-compose.yml` you fetched in step 2 already points at `./config.yaml` and `./projects`:

```yaml
services:
  openrepo-sync:
    image: ghcr.io/opentreecz/openrepo-sync:latest
    volumes:
      - ./config.yaml:/config.yaml:ro
      - ./projects:/projects:ro
    environment:
      - OPENREPO_API_KEY=${OPENREPO_API_KEY}
    command: ["--config", "/config.yaml", "--projects", "/projects"]
    restart: "no"
```

```sh
# Dry run
docker compose run --rm openrepo-sync --dry-run

# Real run
docker compose run --rm openrepo-sync

# Sync a single project only
docker compose run --rm openrepo-sync --project curl
```

### 10. Automate it

`openrepo-sync` performs one sync pass and exits — it is designed to be triggered on a schedule, not run as a long-lived service.

**Cron** (runs every night at 02:00):

```cron
0 2 * * * cd /etc/openrepo-sync && docker compose run --rm openrepo-sync >> /var/log/openrepo-sync.log 2>&1
```

**systemd timer** (alternative to cron):

`/etc/systemd/system/openrepo-sync.service`
```ini
[Unit]
Description=openrepo-sync

[Service]
Type=oneshot
WorkingDirectory=/etc/openrepo-sync
ExecStart=/usr/bin/docker compose run --rm openrepo-sync
```

`/etc/systemd/system/openrepo-sync.timer`
```ini
[Unit]
Description=Run openrepo-sync nightly

[Timer]
OnCalendar=*-*-* 02:00:00
Persistent=true

[Install]
WantedBy=timers.target
```

```sh
systemctl daemon-reload
systemctl enable --now openrepo-sync.timer
systemctl list-timers openrepo-sync.timer   # verify next run time
```

---

## Reference

### Passing the API key inline (without `.env`)

```sh
docker run --rm \
  -e OPENREPO_API_KEY=your_token_here \
  -v ./config.yaml:/config.yaml:ro \
  -v ./projects:/projects:ro \
  ghcr.io/opentreecz/openrepo-sync:latest
```

### Pull a specific version

```sh
docker pull ghcr.io/opentreecz/openrepo-sync:v0.1.0
```

### Verbose / debug logging

```sh
docker run --rm \
  --env-file .env \
  -v ./config.yaml:/config.yaml:ro \
  -v ./projects:/projects:ro \
  ghcr.io/opentreecz/openrepo-sync:latest \
  --verbose
```

Or set `RUST_LOG` for finer-grained filtering (takes precedence over `--verbose`):

```sh
docker run --rm \
  --env-file .env \
  -e RUST_LOG=openrepo=debug,reqwest=warn \
  -v ./config.yaml:/config.yaml:ro \
  -v ./projects:/projects:ro \
  ghcr.io/opentreecz/openrepo-sync:latest
```

---

## Image Details

- **Base image:** `debian:bookworm-slim`
- **Runtime packages:** `ca-certificates`, `dpkg`, `rpm`
- **Runs as:** non-root system user `openrepo`
- **Entrypoint:** `/usr/bin/openrepo-sync`
- **Default CMD:** `--help`
- **download_dir:** not mounted by default — packages are deleted immediately after upload, and the container is ephemeral (`--rm`), so no persistent volume is needed for it

---

## Troubleshooting

| Symptom | Likely cause | Fix |
|---|---|---|
| `Failed to read config file: /config.yaml` | Volume not mounted, or wrong path passed to `--config` | Check the `-v ./config.yaml:/config.yaml:ro` mount and that the file exists on the host |
| `Authentication check failed` | Wrong `api_url`, or `api_key` did not expand | Verify `api_url` has no trailing slash; confirm `OPENREPO_API_KEY` is set in `.env` / `-e` and that `config.yaml` references it as `${OPENREPO_API_KEY}` |
| `Permission denied` reading mounted files | Host file not world/group-readable; container runs as a non-root `openrepo` user | `chmod 644 config.yaml projects/*.yaml` on the host |
| `No project named '<x>' found` | `--project` name doesn't match any `name:` field under `projects/` | Check the `name` field inside the project YAML files, not the filename |
| Project silently skipped | File extension isn't `.yaml`/`.yml` (e.g. still has the `.example` suffix) | Rename it: `mv projects/curl.yaml.example projects/curl.yaml` |
