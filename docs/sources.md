---
layout: page
title: Source Types
permalink: /sources/
---

<p align="center">
  <a href="https://github.com/opentreecz">
    <img src="assets/logo.png" alt="OpenTree" width="96" height="96">
  </a>
</p>

# Source Types

Each project's `source` block specifies where to look for new package versions. The `type` field selects which source driver to use.

---

## `github` — GitHub Releases {#github}

Fetches release assets via the [GitHub Releases API](https://docs.github.com/en/rest/releases/releases).

```yaml
source:
  type: github
  owner: curl               # GitHub organisation or user
  repo: curl                # repository name
  asset_filter: "*.deb"    # optional glob; omit to keep all assets per release
  prerelease: false         # default: false — include pre-releases?
  arch_filter: [amd64, arm64]  # see below
```

### `arch_filter`

When a release publishes assets for multiple architectures (e.g. both `tool_amd64.deb` and `tool_arm64.deb`), `arch_filter` selects the single best-matching asset per release. The list is an ordered preference — the first entry that matches an asset filename wins.

| Setting | Behaviour |
|---|---|
| `arch_filter: [amd64, arm64]` | Prefer amd64, fall back to arm64 **(default)** |
| `arch_filter: [arm64, amd64]` | Prefer arm64 instead |
| `arch_filter: amd64` | Single-string shorthand — download only amd64 |
| `arch_filter: []` | Disable arch filtering — keep all matching assets |

**Aliases:** `amd64`, `x86_64`, and `x86-64` are treated as equivalent. `arm64` and `aarch64` are treated as equivalent. Specifying any one of an alias group matches assets named with any spelling in that group.

If no asset filename matches any arch entry, the first candidate is used as a fallback so no release is silently dropped.

### Fields

| Field | Required | Default | Description |
|---|---|---|---|
| `owner` | Yes | — | GitHub organisation or username |
| `repo` | Yes | — | Repository name |
| `asset_filter` | No | (all assets) | Glob pattern to filter release assets |
| `prerelease` | No | `false` | Include pre-release versions |
| `arch_filter` | No | `[amd64, arm64]` | Architecture preference list |

### Behaviour

- Version is taken from the release `tag_name` (e.g. `v8.5.0` → `8.5.0`)
- Draft releases are always skipped
- Results are paginated (100 per page) until `keep_versions` assets are found
- Unauthenticated requests are subject to GitHub's 60 req/hour rate limit per IP

---

## `deb_repo` — Debian APT Repository {#deb_repo}

Mirrors packages from any standard Debian (APT) repository by fetching and parsing the `Packages.gz` (or plain `Packages`) index.

```yaml
source:
  type: deb_repo
  url: https://nginx.org/packages/debian   # repository base URL

  # Suite(s) to mirror. Single string or list.
  suites: bookworm                  # default: bookworm

  # Component(s). Single string or list.
  components: nginx                 # default: main

  # Architecture(s). Single string or list.
  architectures: [amd64, arm64]    # default: amd64

  # Filter by exact Debian package name (Package: field). Optional.
  package_filter: nginx

  # Filter by filename glob (applied to the Filename basename). Optional.
  filename_filter: "nginx_*.deb"

  # Verify the InRelease/Release GPG signature. Default: true.
  verify_gpg: true

  # GPG public key — URL or inline ASCII-armored key block.
  gpg_key: https://nginx.org/keys/nginx_signing.key
```

### Multiple suites and architectures

All combinations of `suites × components × architectures` are fetched. Results are deduplicated by filename and sorted newest-first before truncation to `keep_versions`.

```yaml
suites: [bookworm, bullseye]
components: [main, contrib]
architectures: [amd64, arm64]
# → fetches 2 × 2 × 2 = 8 Packages indexes
```

### GPG verification

When `verify_gpg: true` (the default), the `InRelease` file is fetched and its GPG signature is verified before any packages are downloaded. The key can be supplied as:

- A URL (`http://` or `https://`) — fetched at sync time
- An inline ASCII-armored key block:

```yaml
gpg_key: |
  -----BEGIN PGP PUBLIC KEY BLOCK-----
  ...
  -----END PGP PUBLIC KEY BLOCK-----
```

Requires `gpg` to be installed on the host (or in the container).

Set `verify_gpg: false` to skip signature verification entirely. When disabled, the `gpg_key` field is ignored and no `InRelease` file is fetched. This is useful when the upstream repository does not provide a signed release file, or when you prefer to manage trust outside of openrepo-sync.

### Fields

| Field | Required | Default | Description |
|---|---|---|---|
| `url` | Yes | — | Repository base URL |
| `suites` | No | `[bookworm]` | Suite(s) — single string or list |
| `components` | No | `[main]` | Component(s) — single string or list |
| `architectures` | No | `[amd64]` | Architecture(s) — single string or list |
| `package_filter` | No | (all packages) | Exact `Package:` field match |
| `filename_filter` | No | (all files) | Glob applied to the filename basename |
| `verify_gpg` | No | `true` | Verify InRelease GPG signature |
| `gpg_key` | No | — | GPG key URL or inline ASCII-armored key |

---

## `direct_url` — Static URL {#direct_url}

A fixed URL where the filename already contains the version string.

```yaml
source:
  type: direct_url
  url: "https://example.com/releases/mypkg-2.1.0.deb"
```

### Fields

| Field | Required | Default | Description |
|---|---|---|---|
| `url` | Yes | — | Full URL to the package file |

### Behaviour

Version is extracted from the filename by regex, matching patterns such as:

- `name-1.2.3.deb` → `1.2.3`
- `name_1.2.3_amd64.deb` → `1.2.3`
- `name-v1.2.3-rc1.tar.gz` → `1.2.3-rc1`

Strings that do not match a semver pattern are stored as a raw version string.

---

## `direct_url_latest` — URL with version in package metadata {#direct_url_latest}

For sources that publish at a fixed URL (e.g. `mypkg-LATEST.deb`) where the filename contains no version. The file is downloaded first; then `dpkg-deb` (`.deb`) or `rpm -qp` (`.rpm`) reads the version from the package metadata. The file is renamed to include the version before upload.

```yaml
source:
  type: direct_url_latest
  url: "https://example.com/releases/mypkg-LATEST.deb"
```

### Fields

| Field | Required | Default | Description |
|---|---|---|---|
| `url` | Yes | — | URL to the always-current package file |

### Behaviour

1. The package is downloaded to a staging directory
2. Version is extracted from package metadata using `dpkg-deb` or `rpm -qp`
3. The file is renamed: `mypkg-LATEST.deb` → `mypkg-2.1.0.deb`
4. The renamed file is uploaded to OpenRepo

### Requirements

| Package format | System tool required |
|---|---|
| `.deb` | `dpkg-deb` (package: `dpkg`) |
| `.rpm` | `rpm` (package: `rpm`) |

---

## `sourceforge` — SourceForge File Releases {#sourceforge}

Scrapes the SourceForge file listing page to discover releases.

```yaml
source:
  type: sourceforge
  project: my-sf-project
  folder: "releases/linux"   # optional subfolder path; omit for root listing
  filename_filter: "*.deb"   # optional glob filter
```

### Fields

| Field | Required | Default | Description |
|---|---|---|---|
| `project` | Yes | — | SourceForge project identifier (from the URL) |
| `folder` | No | (root listing) | Subfolder path within the project's Files section |
| `filename_filter` | No | (all files) | Glob pattern to filter filenames |

### Behaviour

- Fetches `https://sourceforge.net/projects/{project}/files/{folder}/`
- Parses the HTML file listing table
- Files are sorted by detected version number, newest first
- `keep_versions` newest results are returned

---

## Common Fields (all source types)

These fields appear at the project level, not inside `source`:

```yaml
name: my-package          # log output identifier and --project NAME
repo_uid: debian-stable   # target OpenRepo repository UID
keep_versions: 3          # keep 3 newest versions; older ones are pruned
on_conflict: skip         # skip | overwrite | error (default)

source:
  type: ...
```

### `on_conflict`

| Value | Behaviour |
|---|---|
| `error` | Return an error if the package already exists **(default)** |
| `skip` | Silently skip upload if the package already exists |
| `overwrite` | Replace the existing package |
