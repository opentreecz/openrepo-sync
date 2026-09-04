# OpenRepo Ecosystem — Comprehensive Development Plan

> **Date:** 2026-08-31
> **Scope:** openrepo (server) + openrepo-sync (client)
> **Approach:** Contract-First (Approach A)
> **License for new code:** AGPL-3.0 in both repos

## Overview

This plan covers 7 workstreams across both repositories, organized into 4 phases.
Each phase builds on the previous one.

**Workstreams:**
1. Shared API contract (OpenAPI)
2. Fragile conflict detection fix
3. Dead abstraction cleanup
4. Code deduplication
5. Security gap fixes
6. Test gap coverage
7. Observability

**Licensing:** All new code in both repos will be AGPL-3.0. Existing openrepo-sync
code (Apache 2.0) remains under Apache 2.0. New files get AGPL-3.0 headers.

**E2E tests:** Each repo will have its own E2E tests that spin up the other as a
Docker dependency.

---

## Phase 1: Foundation — API Contract & Security (Weeks 1–3) ✅ COMPLETE

### 1.1 OpenAPI Specification (Server) ✅

**Goal:** Generate a machine-readable API contract from the existing DRF views
using `drf-spectacular`. This is the foundation for typed API clients and
contract testing between the two repos.

#### Server Code Changes (8 files in openrepo repo)

| # | File | Change |
|---|------|--------|
| 1 | `web/requirements.txt` | Add `drf-spectacular==0.28.0` |
| 2 | `web/openrepo/settings.py` | Add `"drf_spectacular"` to `INSTALLED_APPS`; `DEFAULT_SCHEMA_CLASS`; `SPECTACULAR_SETTINGS` |
| 3 | `web/openrepo/urls.py` | Add `/api/schema/` and `/api/docs/` URL patterns |
| 4 | `web/repo/api/serializers.py` | Add `UploadResponseSerializer`, `PGPKeyCreateRequestSerializer`, `@extend_schema_field` |
| 5 | `web/repo/api/views.py` | Add `@extend_schema` decorators on 4-6 views |
| 6 | `web/repo/tests/test_openapi.py` | **New** — test schema endpoint (~40 lines) |
| 7 | `.github/workflows/main.yml` | Add schema validation CI step |
| 8 | `web/dev-requirements.txt` | Add `pyyaml` if needed |

#### Documentation Changes in This Repo (7 files)

| File | Changes |
|------|---------|
| `docs/api.md` | Add note at top linking to server's OpenAPI spec at `/api/schema/` and Swagger UI at `/api/docs/`; resolve all 5 "Open Questions" (lines 295-303) |
| `README.md` | Update "Requires" callout (line 17) to mention server provides API docs at `/api/docs/` |
| `CONTRIBUTING.md` | Add row to "What to update" table: "API changes → verify against server OpenAPI spec" |
| `docs/configuration.md` | Add API docs link after API key mention (line 68) |
| `CLAUDE.md` | Add note about OpenAPI spec; update known issue #5 (manual JSON parsing) |
| `ARCHITECTURE_ANALYSIS.md` | Note hardcoded URLs can be validated against spec; note field contracts documented |
| `DEVELOPMENT_PLAN.md` | Mark 1.1 as completed after implementation |

#### Impact on This Repo

Once the server exposes an OpenAPI spec:
- Phase 3.1 (Typed API Client Structs) can derive types from the spec
- Phase 3.6 (OpenAPI Schema Validation in CI) can validate client structs against it
- Known issue #5 (manual JSON parsing in `repo_client.rs`) has a clear resolution path

### 1.2 API Versioning (Server) ✅

**Goal:** Version the API without breaking existing clients.

**Approach:** URL-prefix versioning with backward-compatible alias.

| File | Change |
|------|--------|
| `web/openrepo/urls.py` | Mount API under both `/api/v1/` and `/api/` (alias) |
| `web/repo/api/views.py` | Add `X-OpenRepo-Version` response header via middleware |

**New file:** `web/repo/api/middleware.py` — `VersionHeaderMiddleware` adding
`X-OpenRepo-Version: 2.5.0` to all API responses.

**No breaking change.** All existing clients continue to work via `/api/`.

### 1.3 Structured Error Responses (Server) ✅

**Goal:** Replace string-matched error detection with machine-readable error codes.

**New files:**

| File | Purpose |
|------|---------|
| `web/repo/api/errors.py` | Error codes enum: `PACKAGE_EXISTS`, `REPO_NOT_FOUND`, `INVALID_REPO_TYPE`, `KEY_IN_USE`, etc. |
| `web/repo/api/exception_handler.py` | Custom DRF exception handler returning `{"code": "PACKAGE_EXISTS", "detail": "...", "status": 409}` |

**Key change:** "Package already exists" errors return **HTTP 409 Conflict** (not 400)
with code `PACKAGE_EXISTS`.

### 1.4 Fix Conflict Detection (Client — this repo) ✅

**Goal:** Replace string matching with HTTP status code + error code checking.

| File | Change |
|------|--------|
| `src/repo_client.rs` | Parse error responses as JSON, extract `code` field. Return typed errors. |
| `src/sync.rs` | Match on typed error variants instead of `e.to_string().contains("400")` |

**New file:** `src/errors.rs` — `SyncError` enum with `PackageExists`, `RepoNotFound`,
`AuthFailed`, `ServerError(String)`.

### 1.5 Critical Security Fixes (Server) ✅

**Priority order:**

| # | Issue | Severity | File | Breaking? |
|---|-------|----------|------|-----------|
| 1.5a | Shell injection via `shell=True` | Critical | `base_repo.py:184` | No |
| 1.5b | CSRF disabled for session auth | High | `authentication.py:94-97` | Yes |
| 1.5c | Hardcoded fallback `SECRET_KEY` | High | `settings.py:53-57` | Yes |
| 1.5d | `ALLOWED_HOSTS = ["*"]` | Medium | `settings.py:62-67` | Yes |
| 1.5e | No upload file size limit | Medium | `views.py:312-357` | No |
| 1.5f | Upload status lacks per-user authz | Low | `views.py:360-369` | No |

---

## Phase 2: Architecture Cleanup (Weeks 3–5)

### 2.1 Implement PackageSource Trait (Client — this repo)

**Goal:** Replace dead trait + manual `match` dispatch with proper trait-object dispatch.

| File | Change |
|------|--------|
| `src/sources/mod.rs` | Remove `#[allow(dead_code)]`. Make trait `Send + Sync`. Add `source_name()`. |
| `src/sources/github.rs` | `impl PackageSource for GithubSource` |
| `src/sources/deb_repo.rs` | `impl PackageSource for DebRepoSource` |
| `src/sources/rpm_repo.rs` | `impl PackageSource for RpmRepoSource` |
| `src/sources/direct_url.rs` | `impl PackageSource for DirectUrlSource` and `DirectUrlLatestSource` |
| `src/sources/sourceforge.rs` | `impl PackageSource for SourceforgeSource` |
| `src/sync.rs` | Replace `match` block with factory: `fn build_source(config, client) -> Box<dyn PackageSource>` |

### 2.2 Extract Shared GPG Module (Client — this repo)

**New file:** `src/gpg.rs`

| Function | From | Purpose |
|----------|------|---------|
| `verify_gpg_signature(key, data, sig, mode)` | `deb_repo.rs`, `rpm_repo.rs` | Unified GPG verification |
| `fetch_gpg_key(source, client)` | Both sources | Fetch key from URL or inline |
| `import_gpg_key(data, homedir)` | Both sources | Dearmor + import into temp keyring |

### 2.3 Share reqwest::Client (Client — this repo)

**Goal:** Single HTTP client with shared connection pool.

Create `reqwest::Client` once in `sync_project_inner()`, pass to all source
constructors and `download_package()`. Remove 5 duplicate `Client::builder()` calls.

### 2.4 Fix Server Adapter Abstractions

| File | Change |
|------|--------|
| `web/adapters/file/base_adapter.py` | Convert to `abc.ABC`, use `@abstractmethod` |
| `web/adapters/file/deb_adapter.py` | Fix constructor signature to match base |
| `web/adapters/file/rpm_adapter.py` | Remove commented-out code |
| `web/adapters/repo/base_repo.py` | `NotImplementedError` instead of `Exception`. Add subprocess timeout. |
| `web/adapters/repo/rpm_repo.py` | Merge `_symlink_packages_to_dir` into base class |

### 2.5 Adapter Registry (Server)

**New file:** `web/adapters/registry.py` — Replace `if/elif` chains with dict-based
adapter lookup.

### 2.6 Code Deduplication Summary

| Duplication | Strategy | Effort |
|-------------|----------|--------|
| GPG verification (~200 lines) | Extract to `src/gpg.rs` (2.2) | Small |
| `reqwest::Client` (6 places) | Pass shared client (2.3) | Small |
| `_symlink_packages_to_dir` vs `_copy_packages` | Merge into base class | Small |
| Architecture resolution in deb adapter | Extract and reuse `_get_architectures()` | Trivial |
| Test setUp boilerplate (server) | Extract shared fixtures module | Medium |
| API URL patterns (Rust + Python) | Generate from OpenAPI spec long-term | Medium |
| User-Agent string | Use `env!("CARGO_PKG_VERSION")` via shared client | Trivial |

---

## Phase 3: Test Infrastructure (Weeks 5–7)

### 3.1 Typed API Client Structs (Client — this repo)

Replace `serde_json::Value` parsing with strongly-typed structs:
`PaginatedResponse<T>`, `ApiPackage`, `ApiUploadStatus`.

### 3.2 E2E Tests in Server Repo

New CI workflow + Docker Compose that starts OpenRepo stack, runs openrepo-sync,
verifies packages appear correctly.

**Test scenarios:**
1. First sync — package uploaded
2. Second sync — "up to date"
3. Conflict handling — `on_conflict: skip`
4. Pruning — upload 3 versions, keep 2
5. API contract — verify response shapes match OpenAPI spec

### 3.3 E2E Tests in Client Repo (this repo)

New `tests/e2e/` directory with Rust integration tests behind `#[cfg(feature = "e2e")]`.
Pull OpenRepo Docker image, start stack, run real sync operations.

### 3.4 Fill Server Test Gaps

- RPM upload integration test
- API pagination test
- Generic repo adapter test
- Shared test fixtures module

### 3.5 Fill Client Test Gaps (this repo)

- RPM GPG verification tests
- `on_conflict: Overwrite` test
- Multi-project sync test
- Typed error response parsing tests

### 3.6 OpenAPI Schema Validation in CI

- Server: `python manage.py spectacular --validate --fail-on-warn`
- Client: Validate typed structs match spec via deserialization tests

---

## Phase 4: Observability & Hardening (Weeks 7–9)

### 4.1 Health Check Endpoint (Server)

`GET /api/health/` — no auth, returns DB/worker/version status.

### 4.2 Structured Logging (Server)

Add `structlog` with JSON output, correlation IDs, request tracing.

### 4.3 Prometheus Metrics (Server)

Add `django-prometheus` with custom metrics: package count, upload duration,
build duration, retention deletions.

### 4.4 PGP Key Encryption at Rest (Server)

Fernet encryption for `private_key_pem` and `passphrase` fields.

### 4.5 Rate Limiting (Server)

DRF throttling: 100 req/min user, 20 req/min upload.

### 4.6 Hash Continuity (Both)

Client sends SHA-256 with upload; server verifies on receipt.

### 4.7 Retention Logic Fix (Server)

Wrap in `transaction.atomic()`. Fix N+1 query. Add DB constraints.

### 4.8 Pagination Fix (Server)

Change `PAGE_SIZE` from 2000 to 500 to match `max_page_size`.

---

## Execution Order

```
Phase 1 (Weeks 1-3): Foundation ✅ COMPLETE
  ├── 1.1 OpenAPI spec generation (server) ✅
  ├── 1.2 API versioning (server) ✅
  ├── 1.3 Structured error responses (server) ✅
  ├── 1.4 Fix conflict detection (client) ✅
  └── 1.5 Security fixes (server) ✅
       ├── 1.5a Shell injection ✅
       ├── 1.5b CSRF protection ✅
       ├── 1.5c SECRET_KEY ✅
       ├── 1.5d ALLOWED_HOSTS ✅
       ├── 1.5e Upload size limit ✅
       └── 1.5f Upload status authz ✅

Phase 2 (Weeks 3-5): Architecture
  ├── 2.1 PackageSource trait (client)
  ├── 2.2 GPG module extraction (client)
  ├── 2.3 Shared reqwest::Client (client)
  ├── 2.4 Fix server adapter abstractions (server)
  ├── 2.5 Adapter registry (server)
  └── 2.6 Remaining deduplication (both)

Phase 3 (Weeks 5-7): Testing
  ├── 3.1 Typed API client structs (client) — depends on 1.1
  ├── 3.2 E2E tests in server repo — depends on 1.3
  ├── 3.3 E2E tests in client repo — depends on 1.4, 3.1
  ├── 3.4 Fill server test gaps
  ├── 3.5 Fill client test gaps
  └── 3.6 OpenAPI schema validation in CI — depends on 1.1

Phase 4 (Weeks 7-9): Observability & Hardening
  ├── 4.1 Health check endpoint (server)
  ├── 4.2 Structured logging (server)
  ├── 4.3 Prometheus metrics (server)
  ├── 4.4 PGP key encryption (server)
  ├── 4.5 Rate limiting (server)
  ├── 4.6 Hash continuity (both)
  ├── 4.7 Retention logic fix (server)
  └── 4.8 Pagination fix (server)
```

## File Change Estimate

| Repo | New Files | Modified Files |
|------|-----------|----------------|
| openrepo (server) | ~12 | ~18 |
| openrepo-sync (client) | ~6 | ~12 |

## Risks & Mitigations

| Risk | Mitigation |
|------|------------|
| API versioning breaks existing clients | `/api/` remains as alias |
| CSRF fix breaks frontend | Update Vue.js Axios config simultaneously |
| SECRET_KEY enforcement breaks deployments | Document migration; provide `generate_secret_key` command |
| `shell=False` refactor may break edge cases | Test with real `apt-ftparchive` and `createrepo_c` |
| E2E tests add CI complexity | Use `workflow_dispatch` initially |
| AGPL-3.0 on openrepo-sync may deter contributors | Clear license boundary documentation |
| `drf-spectacular` may not handle custom views | Manual `@extend_schema` for 4 identified views |
| Fernet encryption requires new env var | Make optional initially, warn if not configured |
