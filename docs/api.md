---
layout: page
title: API Reference
permalink: /api/
---

# OpenRepo REST API Reference

This page documents the OpenRepo REST API as used by `openrepo-sync`. The API is built on [Django REST Framework](https://www.django-rest-framework.org/) and serves both the OpenRepo web frontend and CLI clients.

> **Source:** [github.com/openkilt/openrepo](https://github.com/openkilt/openrepo) — verified from `web/repo/api/urls.py`, `views.py`, `serializers.py`, and `cli/openrepo_cli/rest_interface.py`.

---

## Base URL

All endpoints are prefixed with `/api/` (applied by nginx or the top-level Django URL config — not present in individual URL patterns).

```
https://your-openrepo-host/api/
```

---

## Authentication

Every request must include a token in the `Authorization` header:

```
Authorization: Token <api_key>
```

- Authentication backend: `rest_framework.authentication.TokenAuthentication`
- Obtain your API key from `GET /api/whoami` → `api_key` field, or from the OpenRepo web UI user profile

### Example

```sh
curl -H "Authorization: Token abc123" https://openrepo.example.com/api/whoami
```

---

## Permissions

The global permission class is `CustomOpenRepoPermission`:

| Request type | Who can call it |
|---|---|
| Safe methods (GET, HEAD, OPTIONS) | Any authenticated user |
| Write methods (POST, PUT, DELETE) | Superusers only — except the four viewsets below |

**Pass-through viewsets** (non-superusers may write, subject to object-level checks):
- `UploadViewSet`
- `CopyViewSet`
- `RepoViewSet`
- `PackageViewSet`

---

## Response Format

- **Success:** JSON body, or `{}` for operations with no return value
- **Error:** JSON body with error detail, matching the HTTP status code
- **401 Unauthorized:** invalid or missing token

---

## Endpoints

### `GET /api/whoami`

Returns details of the currently authenticated user. No trailing slash.

**Response fields** (`UserDetailSerializer`):

| Field | Type | Description |
|---|---|---|
| `href` | URL | Self-link |
| `username` | string | Username of the authenticated user |
| `is_superuser` | boolean | Whether the user has superuser permissions |
| `email` | string | User email address |
| `api_key` | string | The API token (read-only, `source='auth_token'`) |

**Example:**
```sh
curl -H "Authorization: Token abc123" https://openrepo.example.com/api/whoami
```
```json
{
  "href": "https://openrepo.example.com/api/users/1/",
  "username": "alice",
  "is_superuser": false,
  "email": "alice@example.com",
  "api_key": "abc123..."
}
```

---

### Router-Registered Resources

The following resources are registered with DRF's `DefaultRouter`, generating standard list and detail endpoints:

| Prefix | Resource | Standard endpoints |
|---|---|---|
| `/api/users/` | Users | `GET` list, `GET/PUT/PATCH/DELETE` detail |
| `/api/repos/` | Repositories | `GET` list, `GET/PUT/PATCH/DELETE` detail |
| `/api/signingkeys/` | PGP signing keys | `GET` list, `GET/PUT/PATCH/DELETE` detail |
| `/api/builds/` | Builds | `GET` list, `GET/PUT/PATCH/DELETE` detail |
| `/api/buildlogs/` | Build logs | `GET` list, `GET/PUT/PATCH/DELETE` detail |

> **Note:** `PackageViewSet` exists but is commented out of the router — packages are accessed via the hand-coded URL patterns below.

---

### `GET /api/<repo_uid>/`

Retrieve repository detail.

**Response fields** (`RepoDetailSerializer`):

| Field | Type | Description |
|---|---|---|
| `repo_uid` | string | Unique repository identifier |
| `repo_type` | string | Package type (e.g. `deb`, `rpm`) |
| `package_count` | integer | Number of packages in the repo |
| `href_upload` | URL | Pre-built upload URL for this repo (hypermedia) |
| `href_packages` | URL | Link to the packages listing for this repo |
| `signing_key` | string | PGP signing key UID, if configured |
| `keep_only_latest` | boolean | Whether the repo auto-prunes to one version |
| `last_updated` | datetime | Timestamp of last package change |
| `promote_to` | string | Target repo UID for promotion, if configured |
| `repo_instructions` | string | Human-readable install instructions |
| `write_access` | boolean | Whether the authenticated user can write |

---

### `POST /api/<repo_uid>/upload/`

Upload a package to the repository.

**Request:** `multipart/form-data`

| Field | Required | Description |
|---|---|---|
| `package_file` | Yes | Binary package file. Field name must be exactly `package_file` |
| `overwrite` | No | Accepts `"1"`, `"true"`, or `"yes"` (case-insensitive) to overwrite an existing package |

**Server behaviour:**
- Computes a SHA-512 checksum of the uploaded file
- Deduplicates: if an identical file (by checksum) already exists in another repo, the file is shared on disk rather than stored twice

**Example:**
```sh
curl -X POST \
  -H "Authorization: Token abc123" \
  -F "package_file=@mypkg_1.2.3_amd64.deb" \
  https://openrepo.example.com/api/my-repo/upload/
```

**With overwrite:**
```sh
curl -X POST \
  -H "Authorization: Token abc123" \
  -F "package_file=@mypkg_1.2.3_amd64.deb" \
  -F "overwrite=1" \
  https://openrepo.example.com/api/my-repo/upload/
```

---

### `GET /api/repos/<repo_uid>/packages/`

List all packages in a repository. Response is paginated.

**Response format:**
```json
{
  "count": 42,
  "next": "https://openrepo.example.com/api/repos/my-repo/packages/?page=2",
  "previous": null,
  "results": [...]
}
```

**`results` item fields** (`PackageSummarySerializer`):

| Field | Type |
|---|---|
| `href_package` | URL |
| `package_uid` | string |
| `package_name` | string |
| `filename` | string |
| `architecture` | string |
| `upload_date` | datetime |
| `version` | string |

> `checksum_sha512` and `build_date` are absent from list responses. Use the detail endpoint to retrieve them.

---

### `GET /api/<repo_uid>/pkg/<package_uid>/`

Retrieve details of a specific package.

**Response fields** (`PackageDetailSerializer` — superset of list fields):

| Field | Type | Notes |
|---|---|---|
| `package_uid` | string | |
| `repo_uid` | string | Read-only, sourced from `repo` relation |
| `filename` | string | |
| `version` | string | |
| `architecture` | string | |
| `checksum_sha512` | string | **Not present in list responses** |
| `build_date` | datetime | **Not present in list responses** |
| `upload_date` | datetime | |

---

### `PUT /api/<repo_uid>/pkg/<package_uid>/`

Update package metadata.

---

### `DELETE /api/<repo_uid>/pkg/<package_uid>/`

Delete a package from the repository. Uses `MultipleFieldLookupMixin` with `lookup_fields = ('repo__repo_uid', 'package_uid')`.

**Example:**
```sh
curl -X DELETE \
  -H "Authorization: Token abc123" \
  https://openrepo.example.com/api/my-repo/pkg/abc-def-123/
```

---

### `POST /api/<repo_uid>/pkg/<package_uid>/copy/`

Copy a package to another repository. The exact request body fields (destination repo identifier) were not confirmed in source analysis — inspect `CopyViewSet` directly for the current field name.

---

## Endpoints Used by openrepo-sync

| Method | Path | Purpose |
|---|---|---|
| `GET` | `/api/whoami` | Verify credentials on startup |
| `GET` | `/api/repos/<repo_uid>/packages/` | List packages in a repo (paginated) |
| `POST` | `/api/<repo_uid>/upload/` | Upload a new package |
| `DELETE` | `/api/<repo_uid>/pkg/<package_uid>/` | Delete an old package (pruning) |

---

## Pagination

List endpoints follow DRF's standard pagination format:

```json
{
  "count": 42,
  "next": "https://openrepo.example.com/api/repos/my-repo/packages/?page=2",
  "previous": null,
  "results": [...]
}
```

`openrepo-sync` follows the `next` URL automatically until all pages are fetched.

---

## Error Responses

| Status | Meaning |
|---|---|
| `401 Unauthorized` | Missing or invalid API token |
| `403 Forbidden` | Valid token but insufficient permissions |
| `404 Not Found` | Repository or package does not exist |
| `400 Bad Request` | Invalid request (e.g. missing `package_file` field) |
| `500 Internal Server Error` | Server-side error |

> **Note:** Exact HTTP status codes for successful operations (upload, delete) were not confirmed from server source analysis. Do not assume 201/204 — test against a live instance.

---

## Open Questions

The following API details were not confirmed from source analysis and may require testing against a live instance:

1. Exact request body field for `POST /api/<repo_uid>/pkg/<package_uid>/copy/` (destination repo identifier)
2. HTTP status codes for successful upload and delete
3. Pagination class (`PageNumberPagination` vs `CursorPagination`) and default `page_size`
4. Upload response body schema (does it return the new `package_uid`?)
5. `BuildViewSet` / `BuildLogViewSet` field shapes and whether a build trigger endpoint exists
