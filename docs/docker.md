---
layout: page
title: Docker
permalink: /docker/
---

# Docker

`openrepo-sync` is published as a multi-platform Docker image to the GitHub Container Registry (GHCR). This page is a complete, step-by-step walkthrough for running it in a container.

## Supported Platforms

| Docker platform | Architecture |
|---|---|
| `linux/amd64` | x86-64 |
| `linux/arm64` | ARM 64-bit |
| `linux/arm/v7` | ARM 32-bit hard-float |

---

## Step-by-Step Setup

### 1. Create a working directory

This directory holds your real config and is bind-mounted into the container. Keep it outside of any public git repo because it will contain your OpenRepo API key (via `.env`).

```sh
mkdir -p /etc/openrepo-sync/projects
cd /etc/openrepo-sync
```

### 2. Fetch the config templates

If you cloned the source repo, the templates are already at its root. Otherwise, download them:

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

Set `openrepo.api_url` to your OpenRepo server's base URL. Leave `api_key` as `${OPENREPO_API_KEY}` — the real value is supplied via environment variable.

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

### 5. Add project files

Pick the template matching your upstream source:

| Template | Source type | Use when… |
|---|---|---|
| `github-example.yaml.example` | `github` | Upstream publishes `.deb`/`.rpm` on GitHub Releases |
| `deb-repo-example.yaml.example` | `deb_repo` | Mirror from a Debian APT repository |
| `direct-url-example.yaml.example` | `direct_url` | Fixed URL, filename contains the version |
| `direct-url-latest-example.yaml.example` | `direct_url_latest` | Fixed "LATEST" URL, version only in package metadata |
| `sourceforge-example.yaml.example` | `sourceforge` | Upstream publishes via SourceForge file releases |

```sh
# GitHub Releases example
curl -fsSL -o projects/curl.yaml \
  https://raw.githubusercontent.com/opentreecz/openrepo-sync/main/projects/github-example.yaml.example
$EDITOR projects/curl.yaml

# Debian APT repo example
curl -fsSL -o projects/nginx.yaml \
  https://raw.githubusercontent.com/opentreecz/openrepo-sync/main/projects/deb-repo-example.yaml.example
$EDITOR projects/nginx.yaml
```

### 6. Pull the image

```sh
docker pull ghcr.io/opentreecz/openrepo-sync:latest
```

### 7. Dry run first

Always dry-run before writing anything. It authenticates against OpenRepo and prints every action it *would* take without uploading or deleting.

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

### 8. Run for real

```sh
docker run --rm \
  --env-file .env \
  -v ./config.yaml:/config.yaml:ro \
  -v ./projects:/projects:ro \
  ghcr.io/opentreecz/openrepo-sync:latest \
  --config /config.yaml --projects /projects
```

The container exits `0` on success, `1` if any project errored.

### 9. Switch to Docker Compose (recommended)

```yaml
# docker-compose.yml
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
docker compose run --rm openrepo-sync --dry-run
docker compose run --rm openrepo-sync
docker compose run --rm openrepo-sync --project curl   # single project
```

### 10. Automate with a schedule

`openrepo-sync` performs one sync pass and exits — designed to be triggered on a schedule.

**Cron** (nightly at 02:00):

```cron
0 2 * * * cd /etc/openrepo-sync && docker compose run --rm openrepo-sync >> /var/log/openrepo-sync.log 2>&1
```

**systemd timer:**

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
```

---

## Reference

### Pull a specific version

```sh
docker pull ghcr.io/opentreecz/openrepo-sync:v0.1.18
```

### Pass the API key inline

```sh
docker run --rm \
  -e OPENREPO_API_KEY=your_token_here \
  -v ./config.yaml:/config.yaml:ro \
  -v ./projects:/projects:ro \
  ghcr.io/opentreecz/openrepo-sync:latest
```

### Verbose / debug logging

```sh
# --verbose flag
docker run --rm --env-file .env \
  -v ./config.yaml:/config.yaml:ro \
  -v ./projects:/projects:ro \
  ghcr.io/opentreecz/openrepo-sync:latest --verbose

# Fine-grained RUST_LOG filter (takes precedence over --verbose)
docker run --rm --env-file .env \
  -e RUST_LOG=openrepo=debug,reqwest=warn \
  -v ./config.yaml:/config.yaml:ro \
  -v ./projects:/projects:ro \
  ghcr.io/opentreecz/openrepo-sync:latest
```

---

## Image Details

| Property | Value |
|---|---|
| Base image | `debian:bookworm-slim` |
| Runtime packages | `ca-certificates`, `dpkg`, `rpm`, `gpg` |
| Runs as | Non-root system user `openrepo` (uid 1000) |
| Entrypoint | `/usr/bin/openrepo-sync` |
| Default CMD | `--help` |

**Note:** The image includes `gpg` for InRelease signature verification (`deb_repo` sources). Set `verify_gpg: false` in the project file to skip GPG checks if your upstream does not provide signed releases.

---

## Building Locally from Source

Build the Docker image from source without installing Rust or any other toolchain on your machine. The build uses a multi-stage `Dockerfile.build` that compiles the binary inside a container.

### Build the image

```sh
docker compose build
```

This compiles `openrepo-sync` from source using `Dockerfile.build` and creates a local image. The first build downloads Rust dependencies (~3-5 minutes); subsequent builds with only source code changes take ~20-30 seconds thanks to Docker layer caching.

### Run the locally-built image

```sh
docker compose up                                # run sync
docker compose run --rm openrepo-sync --dry-run  # preview
docker compose run --rm openrepo-sync --version  # check version
```

### Pull vs Build

| Command | Description |
|---|---|
| `docker compose pull` | Download the pre-built image from GHCR (fastest, multi-platform) |
| `docker compose build` | Build from source locally (no toolchain needed on host) |
| `docker compose build --no-cache` | Force a clean rebuild from scratch |
| `docker compose up --build` | Rebuild and run in one step |

### Requirements

| Requirement | Details |
|---|---|
| Docker | 20.10+ with BuildKit support |
| Disk space | ~2 GB temporary (Rust compiler + build artifacts; freed after build) |
| Network | Internet access to download Rust crates on first build |

### Build architecture

The local build produces a native binary for your host architecture only. For multi-platform images (`linux/amd64`, `linux/arm64`, `linux/arm/v7`), use the pre-built images from GHCR:

```sh
docker compose pull
```

---

## Troubleshooting

| Symptom | Likely cause | Fix |
|---|---|---|
| `Failed to read config file: /config.yaml` | Volume not mounted, or wrong `--config` path | Check the `-v ./config.yaml:/config.yaml:ro` mount and that the file exists |
| `Authentication check failed` | Wrong `api_url` or unexpanded `api_key` | Verify `api_url` has no trailing slash; confirm `OPENREPO_API_KEY` is in `.env` and `config.yaml` references it as `${OPENREPO_API_KEY}` |
| `Permission denied` reading mounted files | Host file not world-readable; container runs as non-root `openrepo` user | `chmod 644 config.yaml projects/*.yaml` on the host |
| `No project named '<x>' found` | `--project` name doesn't match any `name:` field in `projects/` | Check the `name:` field inside the YAML files, not the filename |
| Project silently skipped | File still has `.example` suffix | `mv projects/curl.yaml.example projects/curl.yaml` |
| `gpg: command not found` | Custom image without `gpg` installed | Set `verify_gpg: false` in the `deb_repo` config, or install `gpg` in your custom image |
| `docker compose build` fails with "cargo not found" | Using wrong Dockerfile | Ensure `Dockerfile.build` exists and `docker-compose.yml` has `dockerfile: Dockerfile.build` in the `build` block |
