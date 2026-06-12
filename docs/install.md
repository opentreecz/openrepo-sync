---
layout: page
title: Installation
permalink: /install/
---

# Installation

## Requirements

- Rust 1.70 or newer (build only — not needed at runtime)
- A running [OpenRepo](https://github.com/openkilt/openrepo) instance
- `dpkg-deb` (package: `dpkg`) — only required for `direct_url_latest` with `.deb` packages
- `rpm` — only required for `direct_url_latest` with `.rpm` packages

---

## From a Release Package

Download the `.deb` or `.rpm` for your platform from the [latest release](https://github.com/{{ site.github_username }}/openrepo-sync/releases/latest).

### Debian / Ubuntu

```sh
sudo dpkg -i openrepo-sync_0.1.0_amd64.deb
```

### RHEL / Fedora

```sh
sudo rpm -i openrepo-sync-0.1.0-1.x86_64.rpm
```

---

## From Source

```sh
git clone https://github.com/{{ site.github_username }}/openrepo-sync
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

## Docker

```sh
docker pull ghcr.io/{{ site.github_username }}/openrepo-sync:latest
```

See [Docker](../docker/) for full usage.
