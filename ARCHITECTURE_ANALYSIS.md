# openrepo-sync — Architecture Analysis

> **Date:** 2026-08-31
> **Purpose:** Detailed findings from deep code analysis. Reference for future development.

## API Client Analysis (`src/repo_client.rs`)

### RepoClient struct

| Field | Type |
|-------|------|
| `base_url` | `String` |
| `api_key` | `String` |
| `client` | `reqwest::Client` |

Constructed at line 61 with `user_agent("openrepo-sync/0.1")`.

### Methods

#### `whoami()` — line 77
- URL: `{base_url}/api/whoami`
- Method: GET
- Response: `{"username": "..."}` → `UserResponse`
- Errors: 401 → specific message; other → generic

#### `list_packages(repo_uid)` — line 99
- URL: `{base_url}/api/{repo_uid}/packages/`
- Method: GET
- Response: `{"results": [...], "next": "url_or_null"}` — parsed via `serde_json::Value`
- Pagination: follows `next` URL until null (line 172-175)
- Parsed fields: `package_uid`, `package_name`, `filename` (fallback to `package_name`), `architecture`, `version` (fallback to filename extraction, then `Raw("0")`)
- **Does NOT use `/api/repos/{uid}/packages/`** — regression test at line 415

#### `upload_package(repo_uid, path, overwrite)` — line 192
- URL: `{base_url}/api/{repo_uid}/upload/`
- Method: POST multipart
- Body: `package_file` (file) + optional `overwrite=1`
- Response: 2xx → success; 202 → parse `{"task_id": str}`, begin polling
- Error: reads body text, `bail!("Upload failed ({status}): {body}")`

#### `poll_upload_status(task_id, filename)` — line 248
- URL: `{base_url}/api/upload-status/{task_id}/`
- Method: GET
- Response: `{"status": "...", "error_message": "..."}`
- Terminal: `completed` → Ok, `failed` → bail with error_message
- Max attempts: 150 (prod) / 3 (test), interval: 2s (prod) / 10ms (test)

#### `delete_package(repo_uid, package_uid)` — line 308
- URL: `{base_url}/api/{repo_uid}/pkg/{package_uid}/`
- Method: DELETE
- Response: any 2xx → success

### Hardcoded URLs and Magic Strings

| Location | Value |
|----------|-------|
| `repo_client.rs:78` | `"{}/api/whoami"` |
| `repo_client.rs:101` | `"{}/api/{}/packages/"` |
| `repo_client.rs:193` | `"{}/api/{}/upload/"` |
| `repo_client.rs:249` | `"{}/api/upload-status/{}/"` |
| `repo_client.rs:309` | `"{}/api/{}/pkg/{}/"` |
| `repo_client.rs:63` | `"openrepo-sync/0.1"` user-agent |

## Sync Workflow (`src/sync.rs`)

### `sync_project()` — line 17
Wraps `sync_project_inner()`. Errors become `SyncAction::Error`.

### `sync_project_inner()` — line 38

1. **Fetch upstream** (line 46-52): Dispatch to source via `match` on `SourceConfig`
2. **List repo packages** (line 54-60): `client.list_packages()`
3. **Determine uploads** (lines 62-78): Build `HashSet` of filenames + versions, filter
4. **Upload loop** (lines 80-132): Download → upload → handle conflict
5. **Prune** (lines 134-155): Delete old versions beyond `keep_versions`

### Conflict Detection (FRAGILE — lines 102-114)

```rust
// Current fragile code:
e.to_string().contains("400")
    || e.to_string().contains("already exists")
```

This catches:
- HTTP 400 from `repo_client.rs:225`: `"Upload failed (400): ..."`
- Failed task from `repo_client.rs:289`: `"Upload of '...' failed on server: Package X already exists..."`

### Package Existence Check

Dual check on filename AND version:
```rust
!repo_filenames.contains(p.filename.as_str())
    && (version_str == "0" || !repo_versions.contains(&version_str))
```

### Pruning Logic

- `managed_groups()` (line 160): Returns `HashSet<(package_name, architecture)>`
- `prune_candidates()` (line 185): Groups by (name, arch), sorts descending by version, returns packages beyond `keep_versions`
- Only packages in managed groups are pruned

## Data Structures (`src/models.rs`)

### `PackageVersion` (enum)
- `Semver(semver::Version)` — parsed semver
- `Raw(String)` — fallback
- **Ordering flaw:** Mixed `Semver` vs `Raw` comparison uses lexicographic strings. `Semver(2.0.0)` vs `Raw("10.0")` → `"2.0.0" > "10.0"` (wrong).

### `RemotePackage` (struct)
- `filename`, `version`, `download_url`, `sha256: Option`, `package_name: Option`, `architecture: Option`

### `RepoPackage` (struct)
- `package_uid`, `filename`, `package_name`, `architecture`, `version`

## Configuration (`src/config.rs`)

### Source Types (6)

| Variant | Tag | Key Fields |
|---------|-----|------------|
| `Github` | `github` | owner, repo, asset_filter, prerelease, arch_filter |
| `DirectUrl` | `direct_url` | url, sha256 |
| `DirectUrlLatest` | `direct_url_latest` | url, sha256 |
| `Sourceforge` | `sourceforge` | project, folder, filename_filter |
| `DebRepo` | `deb_repo` | url, layout, suites, components, architectures, package_filter, filename_filter, verify_gpg, gpg_key |
| `RpmRepo` | `rpm_repo` | url, package_filter, filename_filter, verify_gpg, gpg_key, architectures |

### `OnConflict` (enum)
- `Error` (default) — propagate upload failure
- `Skip` — catch "already exists" and continue
- `Overwrite` — send `overwrite=1` form field

## Code Duplication Inventory

| What | Where | Lines | Fix |
|------|-------|-------|-----|
| GPG verification | `deb_repo.rs:308-412` + `rpm_repo.rs:396-516` | ~200 | Extract to `src/gpg.rs` |
| `reqwest::Client` construction | 6 locations | ~18 | Share via constructor |
| User-Agent string | `repo_client.rs:63` + `sync.rs:315` | 2 | Use `env!("CARGO_PKG_VERSION")` |

### reqwest::Client Construction Locations

1. `repo_client.rs:61-65` — API calls
2. `sync.rs:313-315` — package downloads
3. `deb_repo.rs:49-51` — deb repo fetching
4. `rpm_repo.rs:101-103` — rpm repo fetching
5. `github.rs:52-54` — GitHub API
6. `sourceforge.rs:22-24` — SourceForge scraping
7. `direct_url.rs:19-21` — direct URL fetching

## Dead Abstraction: PackageSource Trait

`src/sources/mod.rs:11-13`:
```rust
#[allow(dead_code)]
pub trait PackageSource {
    async fn fetch_latest(&self, n: usize) -> Result<Vec<RemotePackage>>;
}
```

- Trait exists but is **never implemented** by any source
- Dispatch is a manual `match` on `SourceConfig` in `sync.rs:209-287`
- Adding a new source requires modifying both `SourceConfig` enum AND the `match` block

## Test Infrastructure

### MockServer (`src/test_util.rs`)

- Minimal blocking TCP server serving canned `MockResponse`s sequentially
- Records request heads for assertions
- **Limitations:**
  - Sequential only — no URL-based routing
  - No request body assertion helpers
  - No concurrent request support
  - `requests()` consumes `self` — callable only once

### Test Coverage by Module

| Module | Tests | Coverage |
|--------|-------|----------|
| `main.rs` | 10 | CLI parsing, integration over MockServer |
| `config.rs` | 20 | All source types, defaults, env vars |
| `models.rs` | 10 | Version parsing, ordering |
| `version.rs` | 10 | Filename extraction, dpkg/rpm |
| `repo_client.rs` | 18 | All API methods, error paths, pagination |
| `sync.rs` | 17 | Dry-run, pruning, conflicts, downloads |
| `github.rs` | 17 | Releases, arch_filter, pagination |
| `direct_url.rs` | 10 | URL parsing, downloads |
| `sourceforge.rs` | 10 | HTML parsing, filtering |
| `deb_repo.rs` | 25+ | Parsing, filters, GPG verification |
| `rpm_repo.rs` | 12 | XML/SQLite parsing, filtering |

### Test Gaps

- No integration test against real OpenRepo server
- No multi-project sync test
- No schedule loop test (`run_scheduled()`)
- No RPM GPG verification test (deb has extensive tests)
- No `on_conflict: Overwrite` upload path test
- No concurrent sync test
- No retry/timeout testing

## Server API Fields Used vs Ignored

### `GET /api/{repo}/packages/` response fields

| Field | Used? | How |
|-------|-------|-----|
| `href_package` | No | HATEOAS link ignored |
| `package_uid` | Yes | Parsed for delete operations |
| `package_name` | Yes | Parsed for pruning groups |
| `filename` | Yes | Parsed (fallback to package_name) |
| `architecture` | Yes | Parsed for pruning groups |
| `upload_date` | No | Ignored |
| `version` | Yes | Parsed (fallback to filename extraction) |

### `GET /api/upload-status/{id}/` response fields

| Field | Used? |
|-------|-------|
| `id` | No |
| `status` | Yes — checked for `completed`/`failed` |
| `filename` | No |
| `filesize` | No |
| `error_message` | Yes — on failure |
| `result_data` | No |
| `created_at` | No |
| `completed_at` | No |

## Hash Algorithm Mismatch

- **Client:** Verifies downloads using SHA-256
- **Server:** Computes and stores SHA-512
- No end-to-end checksum validation between client and server
