---
layout: page
title: Source Types
permalink: /sources/
---

# Source Types

Each project's `source` block specifies where to look for new package versions.

---

## `github` — GitHub Releases

Fetches release assets via the [GitHub Releases API](https://docs.github.com/en/rest/releases/releases).

```yaml
source:
  type: github
  owner: curl              # GitHub organisation or user
  repo: curl               # repository name
  asset_filter: "*.deb"   # optional glob; omit to keep all assets per release
  prerelease: false        # optional, default: false
  arch_filter: [amd64, arm64]  # optional; ordered architecture preference
```

### Fields

| Field | Required | Description |
|---|---|---|
| `owner` | Yes | GitHub organisation or username |
| `repo` | Yes | Repository name |
| `asset_filter` | No | Glob pattern to filter release assets (e.g. `"*_amd64.deb"`) |
| `prerelease` | No | Include pre-release versions. Default: `false` |
| `arch_filter` | No | Ordered architecture preference list. Default: `[amd64, arm64]` |

### Architecture selection (`arch_filter`)

When a release publishes assets for several architectures, `arch_filter` picks **one asset per release**: the first asset whose filename matches the earliest entry in the list wins. If no entry matches any asset, the first candidate is used as a fallback so the release is not silently dropped.

- Accepts a single string (`arch_filter: amd64`) or a list (`arch_filter: [arm64, amd64]`)
- `amd64` / `x86_64` / `x86-64` are treated as aliases; so are `arm64` / `aarch64`
- Set `arch_filter: []` to disable selection and collect every asset that passes `asset_filter` (the pre-0.1.14 behaviour)

### Behaviour

- Version is taken from the release `tag_name` (e.g. `v8.5.0` → `8.5.0`)
- Draft releases are always skipped
- Results are paginated (100 per page) until `keep_versions` assets are found
- Unauthenticated requests are subject to GitHub's 60 req/hour rate limit per IP

---

## `direct_url` — Static URL

A fixed URL where the filename already contains the version string.

```yaml
source:
  type: direct_url
  url: "https://example.com/releases/mypkg-2.1.0.deb"
```

### Behaviour

Version is extracted from the filename by regex, matching patterns such as:

- `name-1.2.3.deb`
- `name_1.2.3_amd64.deb`
- `name-v1.2.3-rc1.tar.gz`

---

## `direct_url_latest` — URL with no version in the filename

For sources that publish under a fixed URL (e.g. `mypkg-LATEST.deb`) where the filename contains no version. The file is downloaded first; then `dpkg-deb` (`.deb`) or `rpm -qp` (`.rpm`) reads the version from the package metadata. The file is renamed to include the version before upload.

```yaml
source:
  type: direct_url_latest
  url: "https://example.com/releases/mypkg-LATEST.deb"
```

### Behaviour

- The package is downloaded to a staging directory
- Version is extracted from package metadata using system tools
- The file is renamed: `mypkg-LATEST.deb` → `mypkg-2.1.0.deb`
- The renamed file is then uploaded to OpenRepo

### Requirements

| Package format | System tool required |
|---|---|
| `.deb` | `dpkg-deb` (package: `dpkg`) |
| `.rpm` | `rpm` (package: `rpm`) |

---

## `sourceforge` — SourceForge

Scrapes the SourceForge file listing page to discover releases.

```yaml
source:
  type: sourceforge
  project: my-sf-project
  folder: "releases/linux"   # optional subfolder path; omit for root listing
  filename_filter: "*.deb"   # optional glob filter
```

### Fields

| Field | Required | Description |
|---|---|---|
| `project` | Yes | SourceForge project identifier (from the URL) |
| `folder` | No | Subfolder path within the project's Files section |
| `filename_filter` | No | Glob pattern to filter filenames |

### Behaviour

- Fetches `https://sourceforge.net/projects/{project}/files/{folder}/`
- Parses the HTML file listing table
- Files are sorted by detected version number, newest first

---

## `deb_repo` — Debian (APT) repository

Mirrors packages from an external Debian repository into OpenRepo. The source fetches the `Packages` index (`Packages.gz`, falling back to plain `Packages`) for each suite/component/architecture combination, filters by package name and/or filename, and uploads the newest versions.

```yaml
source:
  type: deb_repo
  url: https://nginx.org/packages/debian   # repository base URL
  suites: bookworm                         # string or list; default: bookworm
  components: nginx                        # string or list; default: main
  architectures: [amd64, arm64]            # string or list; default: amd64
  package_filter: nginx                    # exact Package: name to sync
  # filename_filter: "nginx_*.deb"        # optional glob on the index filename
  verify_gpg: true                         # default: true
  gpg_key: https://nginx.org/keys/nginx_signing.key
```

### Fields

| Field | Required | Description |
|---|---|---|
| `url` | Yes | Base URL of the Debian repository (the part before `/dists/…`) |
| `suites` | No | Suite(s) to mirror, e.g. `bookworm`. String or list. Default: `bookworm` |
| `components` | No | Component(s), e.g. `main`. String or list. Default: `main` |
| `architectures` | No | Architecture(s), e.g. `amd64`. String or list. Default: `amd64` |
| `package_filter` | No | Exact package name (the `Package:` field in the index) |
| `filename_filter` | No | Glob applied to the basename of the `Filename:` field |
| `verify_gpg` | No | Verify the suite's `InRelease` signature before fetching. Default: `true` |
| `gpg_key` | When `verify_gpg` | GPG public key: an `http(s)://` URL fetched at sync time, or an inline ASCII-armored key block |

### Behaviour

- Every `suites` × `components` × `architectures` combination is fetched and merged
- Duplicate filenames across combinations (e.g. `Architecture: all` packages) are collapsed
- Results are sorted newest-first by version and truncated to `keep_versions`
- The download URL is built from the repository base URL plus the index's `Filename:` field

### GPG verification

With `verify_gpg: true` (the default), the suite's `InRelease` file is verified against `gpg_key` using the system `gpg` binary before any package index is trusted. If `verify_gpg` is `true` but no `gpg_key` is configured, verification is skipped with a debug notice — set an explicit `verify_gpg: false` if the repository is unsigned.

### Requirements

| Condition | System tool required |
|---|---|
| `verify_gpg: true` (default) | `gpg` (package: `gpg` on Debian/Ubuntu, `gnupg2` on RHEL/Fedora) |
