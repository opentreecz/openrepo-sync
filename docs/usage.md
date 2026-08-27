---
layout: page
title: Usage
permalink: /usage/
---

<p align="center">
  <a href="https://github.com/opentreecz">
    <img src="assets/logo.png" alt="OpenTree" width="96" height="96">
  </a>
</p>

# Usage

## Command-Line Reference

```
openrepo-sync [OPTIONS]

Options:
  --config <FILE>     Global config file             [default: config.yaml]
  --projects <DIR>    Per-project YAML directory     [default: projects/]
  --project <NAME>    Sync only the named project
  --dry-run           Preview actions without uploading or deleting
  -v, --verbose       Enable debug logging
  -h, --help          Show help
  -V, --version       Show version
```

---

## Examples

```sh
# Sync all projects with default paths
openrepo-sync

# Sync a single project
openrepo-sync --project curl

# Preview what would change — no writes
openrepo-sync --dry-run

# Verbose output for a single project
openrepo-sync --project curl --verbose

# Non-default config paths
openrepo-sync \
  --config /etc/openrepo-sync/config.yaml \
  --projects /etc/openrepo-sync/projects

# Cron job: show only warnings and errors
RUST_LOG=warn openrepo-sync --config /etc/openrepo-sync/config.yaml
```

---

## Sync Workflow

For each project, in order:

1. Verify authentication via `GET /api/whoami` (once per run, on startup)
2. Fetch the latest `keep_versions` releases from the upstream source
3. List packages currently in the OpenRepo repository
4. Diff by filename **and version** — a package is uploaded only if neither its filename nor its version is already present (packages whose version cannot be detected are compared by filename alone)
5. For each missing package: download → upload → remove local temp file. If OpenRepo rejects the upload as a duplicate, the project's `on_conflict` policy decides: `error` (default), `skip`, or `overwrite`
6. Re-fetch the repo package list and determine the `(package_name, architecture)` groups managed by the current project
7. Delete packages beyond `keep_versions` only within those managed groups, keeping the newest versions per package name and architecture

Projects sharing the same OpenRepo repository UID do not prune each other's packages. Manual uploads outside the current project's managed groups are left untouched.

For shared repositories, run `--dry-run` after changing `keep_versions`, `package_filter`, or `architectures` to confirm only the intended groups would be pruned.

If a project fails, the error is printed to stderr and remaining projects continue. The process exits with code 1 if any project had an error.

---

## Logging and Debugging

`openrepo-sync` uses structured logging via the [`tracing`](https://docs.rs/tracing) crate.

| Mode | What is shown |
|---|---|
| Default (info) | Auth result, per-project status (up-to-date / uploaded / pruned), errors |
| `--verbose` / `-v` | + config paths, URL fetches, package counts, API request URLs, module paths |
| `RUST_LOG=debug` | Same as `--verbose`; `RUST_LOG` takes precedence when set |

### Fine-grained `RUST_LOG` filters

```sh
# All debug output
RUST_LOG=debug openrepo-sync

# Own code at debug, suppress noisy HTTP internals
RUST_LOG=openrepo=debug,reqwest=warn,hyper=warn openrepo-sync

# Trace a single module
RUST_LOG=openrepo_sync::sync=trace openrepo-sync

# Pipe verbose output through a pager
RUST_LOG=debug openrepo-sync --project curl 2>&1 | less
```

---

## Exit Codes

| Code | Meaning |
|---|---|
| `0` | All projects synced successfully |
| `1` | One or more projects encountered an error |

---

## Version Comparison

Versions are parsed as [semver](https://semver.org/) where possible. Tag prefixes such as `v` are stripped before parsing. Pre-release suffixes (e.g. `-rc1`, `-beta.2`) are preserved and compared correctly. Strings that do not parse as semver fall back to lexicographic comparison.
