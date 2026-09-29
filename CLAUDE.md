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
  main.rs          — CLI entry point, scheduler loop, logging, USER_AGENT constant
  config.rs        — YAML config deserialization, ${ENV_VAR} expansion
  models.rs        — PackageVersion, RemotePackage, RepoPackage, SyncResult
  errors.rs        — Typed error types (UploadError, ApiError)
  version.rs       — Version extraction from filenames, dpkg-deb, rpm
  gpg.rs           — Shared GPG verification (clearsigned + detached signatures)
  repo_client.rs   — OpenRepo REST API client (whoami, list, upload, delete)
  sync.rs          — Per-project sync orchestration (fetch -> compare -> upload -> prune)
  test_util.rs     — Custom MockServer + test_client() helper for tests
  sources/
    mod.rs         — PackageSource trait + AnySource enum dispatch
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

1. **~~Fragile conflict detection~~** — ✅ RESOLVED: `sync.rs` now uses typed
   `UploadError::PackageExists` variant. Server returns HTTP 409 + `PACKAGE_EXISTS`
   error code. Backward-compatible fallback for older servers retained.

2. **~~Dead PackageSource trait~~** — ✅ RESOLVED: `AnySource` enum dispatch in
   `sources/mod.rs` with `build_source()` factory in `sync.rs`. The `PackageSource`
   trait is retained as interface documentation.

3. **~~GPG code duplication~~** — ✅ RESOLVED: Extracted shared GPG module to
   `src/gpg.rs` with `verify_clearsigned()` and `verify_detached()` functions.
   Both `deb_repo.rs` and `rpm_repo.rs` delegate to the shared module.

4. **~~reqwest::Client constructed 7 times~~** — ✅ RESOLVED: Single shared
   `reqwest::Client` constructed in `main.rs:run()` and passed through to all
   source constructors and `download_package()`.

5. **~~Manual JSON parsing~~** — ✅ RESOLVED: `repo_client.rs` now uses typed
   `PaginatedResponse<ApiPackage>` structs with `serde::Deserialize` instead of
   `serde_json::Value` with `.get()` chains.

6. **~~Hardcoded User-Agent~~** — ✅ RESOLVED: `USER_AGENT` constant in `main.rs`
   uses `concat!("openrepo-sync/", env!("CARGO_PKG_VERSION"))`. Set once on the
   shared `reqwest::Client`.

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
