# Contributing to openrepo-sync

Thank you for your interest in contributing! This guide covers everything you
need to know to get started.

---

## Table of Contents

- [Code of Conduct](#code-of-conduct)
- [Getting Started](#getting-started)
- [Development Setup](#development-setup)
- [Making Changes](#making-changes)
- [Commit Messages](#commit-messages)
- [Pull Requests](#pull-requests)
- [Code Style](#code-style)
- [Testing](#testing)
- [Documentation](#documentation)
- [Release Process](#release-process)
- [Getting Help](#getting-help)

---

## Code of Conduct

Be respectful, constructive, and collaborative. We welcome contributions from
everyone regardless of experience level.

---

## Getting Started

### Reporting Bugs

Use the [Bug Report](https://github.com/opentreecz/openrepo-sync/issues/new?template=bug_report.yml) template. Include:

- Version (`openrepo-sync --version`)
- Installation method (Docker, .deb, source, etc.)
- Steps to reproduce
- Log output (`--verbose` or `RUST_LOG=debug`)
- Relevant project YAML (redact secrets)

### Suggesting Features

Use the [Feature Request](https://github.com/opentreecz/openrepo-sync/issues/new?template=feature_request.yml) template. Describe:

- The problem or motivation
- Your proposed solution
- Alternatives you've considered
- Example configuration (if applicable)

---

## Development Setup

### Prerequisites

| Tool | Version | Purpose |
|------|---------|---------|
| Rust | 1.70+ | Compilation |
| `dpkg-deb` | any | Testing `direct_url_latest` with .deb files |
| `gpg` | any | Testing `deb_repo` GPG verification |
| Docker | 20.10+ | Building/testing the container image |

### Clone and build

```sh
git clone https://github.com/opentreecz/openrepo-sync
cd openrepo-sync
cargo build
```

### Build with Docker (no Rust needed)

```sh
docker compose build
```

### Run tests

```sh
cargo test --all-targets
```

### Run all CI checks locally

```sh
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-targets --all-features
```

---

## Making Changes

### Branch naming

| Type | Pattern | Example |
|------|---------|---------|
| Bug fix | `fix/<short-description>` | `fix/gpg-homedir` |
| Feature | `feature/<short-description>` | `feature/build` |
| Documentation | `docs/<short-description>` | `docs/update-sources` |
| CI/Infra | `ci/<short-description>` | `ci/add-smoke-test` |

### Workflow

1. Fork the repository (or create a branch if you have write access)
2. Create a feature branch from `main`
3. Make your changes
4. Run all CI checks locally (see above)
5. Commit with a descriptive message (see [Commit Messages](#commit-messages))
6. Push and open a pull request

---

## Commit Messages

We follow [Conventional Commits](https://www.conventionalcommits.org/):

```
<type>: <short summary>

<optional body explaining the change in detail>
```

### Types

| Type | When to use |
|------|-------------|
| `fix` | Bug fix |
| `feat` | New feature |
| `docs` | Documentation only |
| `test` | Adding or updating tests |
| `ci` | CI/CD workflow changes |
| `refactor` | Code change that doesn't fix a bug or add a feature |
| `chore` | Maintenance (deps, formatting, etc.) |

### Examples

```
fix: add --homedir to gpg invocations for container compatibility
feat: add explicit gpg binary availability check
docs: update GPG verification documentation
test: add GPG verification coverage for deb_repo
ci: add Docker build smoke test
```

### Version bumps

The release workflow automatically bumps the patch version on every push to
`main`. To bump minor or major, include `[minor]` or `[major]` in your merge
commit message.

---

## Pull Requests

### Before submitting

- [ ] Code compiles without errors (`cargo check`)
- [ ] All tests pass (`cargo test --all-targets`)
- [ ] No clippy warnings (`cargo clippy -- -D warnings`)
- [ ] Code is formatted (`cargo fmt --check`)
- [ ] Documentation updated (if applicable)
- [ ] Man page updated (if CLI changes)
- [ ] Example files updated (if configuration changes)

### PR guidelines

- Fill out the [pull request template](.github/pull_request_template.md)
- Keep PRs focused — one logical change per PR
- Split large changes into multiple commits with clear messages
- Link related issues (`Fixes #123` or `Relates to #456`)
- PRs are automatically reviewed by the OpenCode AI agent

### Review process

1. Automated checks run (formatting, clippy, tests, Docker build)
2. OpenCode AI reviews the PR for code quality and potential issues
3. A maintainer reviews and approves
4. The PR is merged (squash or merge commit depending on size)

---

## Code Style

- Follow standard Rust conventions (`cargo fmt` enforces formatting)
- Use `anyhow::Result` for fallible functions
- Use `tracing` macros (`debug!`, `info!`, `warn!`) for logging
- Keep functions focused and reasonably sized
- Add doc comments for public functions and types
- Prefer descriptive variable names over abbreviations

### Project structure

```
src/
  main.rs           — CLI parsing, entry point
  config.rs         — YAML config deserialization
  models.rs         — Shared types (RemotePackage, PackageVersion)
  repo_client.rs    — OpenRepo REST API client
  sync.rs           — Orchestration (fetch → compare → upload → prune)
  version.rs        — Version extraction from filenames/packages
  test_util.rs      — Shared test utilities (MockServer, helpers)
  sources/
    mod.rs          — Source trait
    github.rs       — GitHub Releases source
    deb_repo.rs     — Debian APT repository source
    direct_url.rs   — Static/LATEST URL sources
    sourceforge.rs  — SourceForge source
```

---

## Testing

### Running tests

```sh
cargo test --all-targets --all-features
```

### Writing tests

- Tests live in `#[cfg(test)] mod tests` at the bottom of each source file
- Use the custom `MockServer` from `test_util.rs` for HTTP mocking
- Use `with_url()` / `with_api_base()` test-only methods to redirect to mock
- For tests requiring system tools (`gpg`, `dpkg-deb`), check availability
  and skip gracefully:

```rust
use crate::test_util::gpg_available;

#[tokio::test]
async fn my_gpg_test() {
    if !gpg_available() {
        eprintln!("skipping: gpg not available");
        return;
    }
    // ...
}
```

### Coverage

Run coverage locally with [cargo-tarpaulin](https://github.com/xd009642/tarpaulin):

```sh
cargo tarpaulin --out Html --output-dir coverage/
open coverage/tarpaulin-report.html
```

---

## Documentation

### What to update

| Change type | Files to update |
|-------------|----------------|
| New CLI option | `man/man1/openrepo-sync.1`, `docs/usage.md`, `README.md` |
| New source type | `docs/sources.md`, `docs/configuration.md`, `README.md`, new example in `projects/` |
| New config field | `docs/sources.md` or `docs/configuration.md`, relevant example file |
| Docker changes | `docs/docker.md`, `docs/install.md` |
| Any user-facing change | `README.md` (if significant) |

### Documentation locations

| Location | Purpose |
|----------|---------|
| `README.md` | Project overview, quick start |
| `docs/*.md` | Full documentation (GitHub Pages via Jekyll) |
| `man/man1/openrepo-sync.1` | Unix man page |
| `projects/*.yaml.example` | Example project templates |
| `config.yaml.example` | Global config template |

### GitHub Pages

Documentation is published automatically to
https://opentreecz.github.io/openrepo-sync/ on every push to `main` that
changes files in `docs/`.

---

## Release Process

Releases are fully automated:

1. Every push to `main` triggers the Release workflow
2. Version is auto-bumped (patch by default, `[minor]`/`[major]` keywords in commit)
3. Static binaries are cross-compiled (amd64, arm64, armhf)
4. `.deb` and `.rpm` packages are built
5. A GitHub Release is created with all artifacts
6. The Docker workflow builds and pushes multi-platform images to GHCR

**You don't need to manually tag, bump versions, or publish releases.**

---

## Getting Help

- Open a [Discussion](https://github.com/opentreecz/openrepo-sync/discussions) for questions
- Check the [documentation](https://opentreecz.github.io/openrepo-sync/)
- Look at existing [issues](https://github.com/opentreecz/openrepo-sync/issues) and [PRs](https://github.com/opentreecz/openrepo-sync/pulls)

Thank you for contributing!
