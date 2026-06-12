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
```

### Fields

| Field | Required | Description |
|---|---|---|
| `owner` | Yes | GitHub organisation or username |
| `repo` | Yes | Repository name |
| `asset_filter` | No | Glob pattern to filter release assets (e.g. `"*_amd64.deb"`) |
| `prerelease` | No | Include pre-release versions. Default: `false` |

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
