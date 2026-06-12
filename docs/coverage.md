---
layout: page
title: Test Coverage
permalink: /coverage/
---

# Test Coverage

Coverage is measured on every CI run using [cargo-tarpaulin](https://github.com/xd009642/tarpaulin) and reported to [Codecov](https://codecov.io).

---

## Current Results

| File | Covered | Total | Coverage |
|---|---|---|---|
| `src/models.rs` | 15 | 15 | **100%** |
| `src/sources/direct_url.rs` | 31 | 61 | **51%** |
| `src/version.rs` | 10 | 27 | **37%** |
| `src/config.rs` | 6 | 29 | **21%** |
| `src/main.rs` | 0 | 56 | 0% |
| `src/repo_client.rs` | 0 | 85 | 0% |
| `src/sources/github.rs` | 0 | 34 | 0% |
| `src/sources/sourceforge.rs` | 0 | 46 | 0% |
| `src/sync.rs` | 0 | 88 | 0% |
| **Total** | **62** | **441** | **14%** |

---

## What Is Covered

Tests focus on pure logic that can run without network access or external processes:

| Module | What is tested |
|---|---|
| `models` | `PackageVersion::parse` — semver, `v`-prefix stripping, pre-release, raw fallback; ordering and sort; Display |
| `version` | `extract_version_from_filename` — dash/underscore/`v`-prefix/pre-release/build-metadata patterns, no-version returns `None`, unsupported extension error |
| `sources::direct_url` | `url_filename` — query-string stripping; `rename_with_version` — LATEST replacement, version insertion, no-extension; `fetch_static_url` — version parsed from URL, fallback to `Raw("0")` |
| `config` | All four source type deserialisations, GitHub defaults, global config with and without `download_dir`, `${ENV_VAR}` expansion |

---

## What Is Not Covered (and Why)

The following modules require either a live network or external system binaries and cannot be covered by unit tests alone:

| Module | Reason |
|---|---|
| `src/repo_client.rs` | All methods make real HTTP calls to an OpenRepo server |
| `src/sources/github.rs` | Fetches the GitHub Releases API over HTTPS |
| `src/sources/sourceforge.rs` | Scrapes SourceForge HTML over HTTPS |
| `src/sync.rs` | Orchestrates all of the above; requires a running server |
| `src/main.rs` | CLI entry point; tested end-to-end rather than unit |
| `version.rs` — dpkg/rpm functions | Invoke `dpkg-deb` and `rpm` system binaries |
| `direct_url.rs` — `fetch_latest_url` | Downloads a real file before version extraction |

To increase coverage for these modules, integration tests against a local OpenRepo instance or HTTP mocking (e.g. [`wiremock`](https://crates.io/crates/wiremock)) would be needed.

---

## Running Coverage Locally

```sh
# Install tarpaulin (once)
cargo install cargo-tarpaulin

# Run with terminal output
cargo tarpaulin --out Stdout

# Run and generate HTML report
cargo tarpaulin --out Html --output-dir coverage/
open coverage/tarpaulin-report.html

# Run and generate Lcov (for editor integration)
cargo tarpaulin --out Lcov --output-dir coverage/
```

---

## CI Integration

Coverage runs automatically on every push and pull request as part of the `Coverage` job in [ci.yml](https://github.com/{{ site.github_username }}/openrepo-sync/blob/main/.github/workflows/ci.yml).

The job:
1. Installs `cargo-tarpaulin` via [`taiki-e/install-action`](https://github.com/taiki-e/install-action) (cached binary, no recompile)
2. Runs `cargo tarpaulin` producing Cobertura XML and Lcov output
3. Uploads results to [Codecov](https://codecov.io) (requires `CODECOV_TOKEN` secret)
4. Uploads the raw report files as a 30-day CI artifact

To enable Codecov: sign in at [codecov.io](https://codecov.io) with your GitHub account, add the repository, and set the `CODECOV_TOKEN` repository secret in **Settings → Secrets → Actions**.
