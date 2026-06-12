---
layout: page
title: Docker
permalink: /docker/
---

# Docker

`openrepo-sync` is published as a multi-platform Docker image to the GitHub Container Registry (GHCR).

## Supported Platforms

| Docker platform | Architecture |
|---|---|
| `linux/amd64` | x86-64 |
| `linux/arm64` | ARM 64-bit |
| `linux/arm/v7` | ARM 32-bit hard-float |

---

## Pull

```sh
# Latest release
docker pull ghcr.io/opentreecz/openrepo-sync:latest

# Specific version
docker pull ghcr.io/opentreecz/openrepo-sync:v0.1.0
```

---

## Run

Mount your `config.yaml` and `projects/` directory as read-only volumes:

```sh
docker run --rm \
  -v ./config.yaml:/config.yaml:ro \
  -v ./projects:/projects:ro \
  ghcr.io/opentreecz/openrepo-sync:latest \
  --config /config.yaml --projects /projects
```

### Passing the API key via environment variable

```sh
docker run --rm \
  -e OPENREPO_API_KEY=your_token_here \
  -v ./config.yaml:/config.yaml:ro \
  -v ./projects:/projects:ro \
  ghcr.io/opentreecz/openrepo-sync:latest
```

Your `config.yaml` should reference `${OPENREPO_API_KEY}`:

```yaml
openrepo:
  api_url: "https://openrepo.example.com"
  api_key: "${OPENREPO_API_KEY}"
```

### Verbose output

```sh
docker run --rm \
  -v ./config.yaml:/config.yaml:ro \
  -v ./projects:/projects:ro \
  ghcr.io/opentreecz/openrepo-sync:latest \
  --verbose
```

---

## Docker Compose

A `docker-compose.yml` is included in the repository. Copy it alongside your `config.yaml` and `projects/` directory:

```sh
cp docker-compose.yml /etc/openrepo-sync/
cd /etc/openrepo-sync/
```

Set the API key in a `.env` file (never commit this file):

```sh
echo "OPENREPO_API_KEY=your_token_here" > .env
```

Run once:

```sh
docker compose run --rm openrepo-sync
```

Run for a specific project only:

```sh
docker compose run --rm openrepo-sync --project curl
```

Dry run:

```sh
docker compose run --rm openrepo-sync --dry-run
```

Schedule via cron (runs every night at 02:00):

```cron
0 2 * * * cd /etc/openrepo-sync && docker compose run --rm openrepo-sync >> /var/log/openrepo-sync.log 2>&1
```

---

## Image Details

- **Base image:** `debian:bookworm-slim`
- **Runtime packages:** `ca-certificates`, `dpkg`, `rpm`
- **Runs as:** non-root system user `openrepo`
- **Entrypoint:** `/usr/bin/openrepo-sync`
- **Default CMD:** `--help`
