---
layout: page
title: Installation
permalink: /install/
---

<p align="center">
  <a href="https://github.com/opentreecz">
    <img src="assets/logo.png" alt="OpenTree" width="96" height="96">
  </a>
</p>

# Installation

## Requirements

| Requirement | When needed |
|---|---|
| A running [OpenRepo](https://github.com/opentreecz/openrepo) instance | Always |
| `dpkg-deb` (package: `dpkg`) | `direct_url_latest` with `.deb` packages |
| `rpm` (package: `rpm`) | `direct_url_latest` with `.rpm` packages |
| `gpg` | `deb_repo` with `verify_gpg: true` |

Rust is only needed to build from source — not at runtime.

---

## From a Release Package

Download the `.deb` or `.rpm` for your platform from the [latest release](https://github.com/opentreecz/openrepo-sync/releases/latest).

### Debian / Ubuntu

```sh
# amd64 (x86-64)
sudo dpkg -i openrepo-sync_*_amd64.deb

# ARM 64-bit
sudo dpkg -i openrepo-sync_*_arm64.deb

# ARM hard-float (armhf)
sudo dpkg -i openrepo-sync_*_armhf.deb
```

### RHEL / Fedora

```sh
# x86_64
sudo rpm -i openrepo-sync-*-1.x86_64.rpm

# aarch64
sudo rpm -i openrepo-sync-*-1.aarch64.rpm

# armv7hl
sudo rpm -i openrepo-sync-*-1.armv7hl.rpm
```

---

## From Source

Requires Rust 1.70 or newer.

```sh
git clone https://github.com/opentreecz/openrepo-sync
cd openrepo-sync
cargo build --release
install -m755 target/release/openrepo-sync /usr/local/bin/
```

### Install the man page

```sh
install -Dm644 man/man1/openrepo-sync.1 /usr/local/share/man/man1/openrepo-sync.1
mandb
man openrepo-sync
```

---

## Build from Source with Docker

If you don't have Rust installed, you can build using Docker — no local toolchain required:

```sh
git clone https://github.com/opentreecz/openrepo-sync
cd openrepo-sync
docker compose build
```

This uses a multi-stage `Dockerfile.build` that compiles the project inside a Rust container and produces a minimal runtime image identical to the published one.

```sh
docker compose run --rm openrepo-sync --version   # verify
docker compose run --rm openrepo-sync --dry-run   # preview sync
```

See [Docker — Building Locally](../docker/#building-locally-from-source) for full details.

---

## Docker

```sh
docker pull ghcr.io/opentreecz/openrepo-sync:latest
```

Supported platforms: `linux/amd64`, `linux/arm64`, `linux/arm/v7`.

See [Docker](../docker/) for the full setup walkthrough.

---

## Verify the installation

```sh
openrepo-sync --version
openrepo-sync --help
```
