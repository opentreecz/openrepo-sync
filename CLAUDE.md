# OpenRepo-Sync — Agent Knowledge Base

## Project Overview

openrepo-sync is a Rust CLI tool that automatically synchronizes packages from
upstream sources into an OpenRepo server via its REST API. It is the companion
client to the OpenRepo server (Django/Vue.js).

- **Language:** Rust (edition 2024, MSRV 1.85)
- **Async runtime:** Tokio
- **HTTP client:** reqwest (rustls-tls)
- **CLI:** clap v4 (derive)
- **Version:** 0.1.44
- **License:** Apache 2.0 (existing code), AGPL-3.0 (new code from 2026-08-31)
- **CI coverage threshold:** 80% (actual ~90%)

## Architecture

```
src/
  main.rs          — CLI entry point, scheduler loop, logging
  config.rs        — YAML config deserialization, ${ENV_VAR} expansion
  models.rs        — PackageVersion, RemotePackage, RepoPackage, SyncResult
  version.rs       — Version extraction from filenames, dpkg-deb, rpm
  repo_client.rs   — OpenRepo REST API client (whoami, list, upload, delete)
  sync.rs          — Per-project sync orchestration (fetch -> compare -> upload -> prune)
  test_util.rs     — Custom MockServer for tests
  sources/
    mod.rs         — PackageSource trait (currently UNUSED — dead code)
    github.rs      — GitHub Releases API source
    deb_repo.rs    — Debian APT repository source
    rpm_repo.rs    — RPM (YUM/DNF) repository source
    direct_url.rs  — Static URL and LATEST URL sources
    sourceforge.rs — SourceForge file listing scraper
```

## API Endpoints Used

The client uses 5 of the server's 14 endpoint groups:

| Method | URL | Purpose |
|--------|-----|---------|
| GET | `/api/whoami` | Auth check |
| GET | `/api/{repo_uid}/packages/` | List packages (paginated) |
| POST | `/api/{repo_uid}/upload/` | Upload package (multipart) |
| GET | `/api/upload-status/{task_id}/` | Poll async upload status |
| DELETE | `/api/{repo_uid}/pkg/{package_uid}/` | Delete package |

Auth: `Authorization: Token <api_key>` header on all requests.

> The server now provides an auto-generated OpenAPI spec at `/api/schema/` and
> interactive Swagger UI at `/api/docs/`. These are the canonical API contract.

## Known Issues (to fix per DEVELOPMENT_PLAN.md)

1. **Fragile conflict detection** — `sync.rs:102-105` matches error strings with
   `contains("400")` and `contains("already exists")`. Must be replaced with typed
   error codes once server returns structured errors.

2. **Dead PackageSource trait** — `sources/mod.rs` defines the trait but no source
   implements it. Dispatch is a manual `match` in `sync.rs:209-287`.

3. **GPG code duplication** — ~200 lines duplicated between `deb_repo.rs:308-412`
   and `rpm_repo.rs:396-516`. Should be extracted to `src/gpg.rs`.

4. **reqwest::Client constructed 6 times** — Should be shared via constructor injection.

5. **Manual JSON parsing** — `repo_client.rs` uses `serde_json::Value` with `.get()`
   chains instead of typed deserialization. Now that the server exposes an OpenAPI
   spec at `/api/schema/`, typed structs can be derived from it (Phase 3.1).

6. **Hardcoded User-Agent** — `"openrepo-sync/0.1"` at `repo_client.rs:63` and
   `sync.rs:315`. Should use `env!("CARGO_PKG_VERSION")`.

## Testing

- All tests are inline (`#[cfg(test)] mod tests`)
- Custom `MockServer` in `test_util.rs` — blocking TCP server with canned responses
- Tests requiring `dpkg-deb` or `gpg` gracefully skip when unavailable
- No E2E tests against a real OpenRepo server (planned)
- No tests for `run_scheduled()` (infinite loop)

## Build & Test Commands

```bash
cargo build --release
cargo test --all-targets --all-features
cargo clippy --all-targets --all-features -- -D warnings
cargo fmt --check
cargo tarpaulin --fail-under 80
```

## Configuration

- Global config: `config.yaml` (API URL, API key, download dir, schedule)
- Per-project: `projects/*.yaml` (name, repo_uid, keep_versions, source config)
- Env var expansion: `${ENV_VAR}` in any YAML value
- 6 source types: `github`, `deb_repo`, `rpm_repo`, `direct_url`, `direct_url_latest`, `sourceforge`

## Related Repository

- **OpenRepo server:** `../openrepo/` (or `github.com/opentreecz/openrepo`)
- See `../openrepo/DEVELOPMENT_PLAN.md` for server-side changes
