use anyhow::{Context, Result};
use serde::Deserialize;
use tracing::debug;

use crate::models::{PackageVersion, RemotePackage};

/// Architecture names considered equivalent to amd64 for priority matching.
const AMD64_ALIASES: &[&str] = &["amd64", "x86_64", "x86-64"];

/// Architecture names considered equivalent to arm64 for priority matching.
const ARM64_ALIASES: &[&str] = &["arm64", "aarch64"];

#[derive(Debug)]
pub struct GithubSource {
    pub owner: String,
    pub repo: String,
    pub asset_filter: Option<glob::Pattern>,
    pub prerelease: bool,
    /// Ordered architecture preference list. When a release has assets for
    /// multiple architectures, the first arch in this list that appears in an
    /// asset filename wins. Empty means "accept everything" (old behaviour).
    pub arch_filter: Vec<String>,
    api_base: String,
    client: reqwest::Client,
}

#[derive(Debug, Deserialize)]
struct Release {
    tag_name: String,
    prerelease: bool,
    draft: bool,
    assets: Vec<ReleaseAsset>,
}

#[derive(Debug, Deserialize)]
struct ReleaseAsset {
    name: String,
    browser_download_url: String,
}

impl GithubSource {
    pub fn new(
        owner: &str,
        repo: &str,
        asset_filter: Option<&str>,
        prerelease: bool,
        arch_filter: Vec<String>,
    ) -> Result<Self> {
        let pattern = asset_filter
            .map(|f| glob::Pattern::new(f).context("Invalid asset_filter glob pattern"))
            .transpose()?;
        let client = reqwest::Client::builder()
            .user_agent("openrepo-sync/0.1")
            .build()?;
        Ok(Self {
            owner: owner.to_string(),
            repo: repo.to_string(),
            asset_filter: pattern,
            prerelease,
            arch_filter,
            api_base: "https://api.github.com".to_string(),
            client,
        })
    }

    /// Returns the priority index of `arch` within `arch_filter` (lower = higher priority),
    /// or `None` if `arch_filter` is empty (accept-all mode).
    fn arch_priority(&self, asset_name: &str) -> Option<usize> {
        if self.arch_filter.is_empty() {
            return None;
        }
        let name_lower = asset_name.to_lowercase();
        self.arch_filter.iter().enumerate().find_map(|(i, arch)| {
            let arch_lower = arch.to_lowercase();
            let is_amd64_entry = AMD64_ALIASES
                .iter()
                .any(|a| a.eq_ignore_ascii_case(&arch_lower));
            let is_arm64_entry = ARM64_ALIASES
                .iter()
                .any(|a| a.eq_ignore_ascii_case(&arch_lower));
            if name_lower.contains(&arch_lower)
                || (is_amd64_entry && AMD64_ALIASES.iter().any(|a| name_lower.contains(*a)))
                || (is_arm64_entry && ARM64_ALIASES.iter().any(|a| name_lower.contains(*a)))
            {
                Some(i)
            } else {
                None
            }
        })
    }

    /// Point the source at a different API host (tests only).
    #[cfg(test)]
    fn with_api_base(mut self, base: &str) -> Self {
        self.api_base = base.to_string();
        self
    }

    pub async fn fetch_latest(&self, n: usize) -> Result<Vec<RemotePackage>> {
        let base_url = format!(
            "{}/repos/{}/{}/releases",
            self.api_base, self.owner, self.repo
        );

        let mut packages = Vec::new();
        let mut page = 1u32;

        // Paginate until we have enough packages or exhaust all releases
        'outer: loop {
            let url = format!("{}?page={}&per_page=100", base_url, page);
            debug!("Fetching GitHub releases from {}", url);

            let releases: Vec<Release> = self
                .client
                .get(&url)
                .header("Accept", "application/vnd.github+json")
                .send()
                .await
                .context("Failed to fetch GitHub releases")?
                .error_for_status()
                .context("GitHub API error")?
                .json()
                .await
                .context("Failed to parse GitHub releases")?;

            if releases.is_empty() {
                break;
            }

            if self.collect_release_packages(releases, &mut packages, n) {
                break 'outer;
            }

            page += 1;
        }

        Ok(packages)
    }

    /// Append matching assets from `releases` to `packages`, skipping drafts
    /// and (unless enabled) prereleases. Returns true once `n` packages have
    /// been collected and pagination can stop.
    fn collect_release_packages(
        &self,
        releases: Vec<Release>,
        packages: &mut Vec<RemotePackage>,
        n: usize,
    ) -> bool {
        for release in releases {
            if release.draft {
                continue;
            }
            if release.prerelease && !self.prerelease {
                continue;
            }
            let version = PackageVersion::parse(&release.tag_name);

            // Apply asset_filter first, then arch selection.
            let candidates: Vec<&ReleaseAsset> = release
                .assets
                .iter()
                .filter(|a| {
                    self.asset_filter
                        .as_ref()
                        .is_none_or(|p| p.matches(&a.name))
                })
                .collect();

            // When arch_filter is non-empty, pick the single best-matching asset
            // per release. "Best" = lowest priority index in arch_filter. Ties
            // are broken by asset order (first wins). If no asset matches any
            // arch, fall back to the first candidate so we never silently drop
            // a release entirely.
            let selected: Vec<&ReleaseAsset> = if self.arch_filter.is_empty() {
                candidates
            } else {
                let best = candidates
                    .iter()
                    .copied()
                    .min_by_key(|a| self.arch_priority(&a.name).unwrap_or(usize::MAX));
                best.into_iter().collect()
            };

            for asset in selected {
                packages.push(RemotePackage {
                    filename: asset.name.clone(),
                    version: version.clone(),
                    download_url: asset.browser_download_url.clone(),
                });
                if packages.len() >= n {
                    return true;
                }
            }
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn asset(name: &str) -> ReleaseAsset {
        ReleaseAsset {
            name: name.to_string(),
            browser_download_url: format!("https://example.com/{}", name),
        }
    }

    fn release(tag: &str, prerelease: bool, draft: bool, assets: Vec<ReleaseAsset>) -> Release {
        Release {
            tag_name: tag.to_string(),
            prerelease,
            draft,
            assets,
        }
    }

    fn collect(source: &GithubSource, releases: Vec<Release>, n: usize) -> Vec<RemotePackage> {
        let mut packages = Vec::new();
        source.collect_release_packages(releases, &mut packages, n);
        packages
    }

    fn new_no_arch(asset_filter: Option<&str>, prerelease: bool) -> GithubSource {
        GithubSource::new("acme", "tool", asset_filter, prerelease, vec![]).unwrap()
    }

    fn new_default_arch() -> GithubSource {
        GithubSource::new(
            "acme",
            "tool",
            None,
            false,
            vec!["amd64".to_string(), "arm64".to_string()],
        )
        .unwrap()
    }

    #[test]
    fn invalid_asset_filter_is_rejected() {
        let err = GithubSource::new("acme", "tool", Some("[bad"), false, vec![]).unwrap_err();
        assert!(err.to_string().contains("Invalid asset_filter"));
    }

    #[test]
    fn collects_assets_with_parsed_version() {
        let source = new_no_arch(None, false);
        let pkgs = collect(
            &source,
            vec![release(
                "v1.2.3",
                false,
                false,
                vec![asset("tool_1.2.3_amd64.deb")],
            )],
            10,
        );
        assert_eq!(pkgs.len(), 1);
        assert_eq!(pkgs[0].version, PackageVersion::parse("1.2.3"));
        assert_eq!(pkgs[0].filename, "tool_1.2.3_amd64.deb");
        assert_eq!(
            pkgs[0].download_url,
            "https://example.com/tool_1.2.3_amd64.deb"
        );
    }

    #[test]
    fn drafts_are_skipped() {
        let source = new_no_arch(None, false);
        let pkgs = collect(
            &source,
            vec![release("v9.9.9", false, true, vec![asset("draft.deb")])],
            10,
        );
        assert!(pkgs.is_empty());
    }

    #[test]
    fn prereleases_skipped_by_default() {
        let source = new_no_arch(None, false);
        let pkgs = collect(
            &source,
            vec![
                release("v2.0.0-rc1", true, false, vec![asset("rc.deb")]),
                release("v1.0.0", false, false, vec![asset("stable.deb")]),
            ],
            10,
        );
        assert_eq!(pkgs.len(), 1);
        assert_eq!(pkgs[0].filename, "stable.deb");
    }

    #[test]
    fn prereleases_included_when_enabled() {
        let source = new_no_arch(None, true);
        let pkgs = collect(
            &source,
            vec![release("v2.0.0-rc1", true, false, vec![asset("rc.deb")])],
            10,
        );
        assert_eq!(pkgs.len(), 1);
    }

    #[test]
    fn asset_filter_selects_matching_assets_only() {
        let source = new_no_arch(Some("*.deb"), false);
        let pkgs = collect(
            &source,
            vec![release(
                "v1.0.0",
                false,
                false,
                vec![asset("tool.rpm"), asset("tool.deb"), asset("tool.tar.gz")],
            )],
            10,
        );
        assert_eq!(pkgs.len(), 1);
        assert_eq!(pkgs[0].filename, "tool.deb");
    }

    // With arch_filter empty, all assets per release are collected (old behaviour).
    #[test]
    fn stops_after_n_packages() {
        let source = new_no_arch(None, false);
        let mut packages = Vec::new();
        let done = source.collect_release_packages(
            vec![
                release("v3.0.0", false, false, vec![asset("a.deb"), asset("b.deb")]),
                release("v2.0.0", false, false, vec![asset("c.deb")]),
            ],
            &mut packages,
            2,
        );
        assert!(done);
        assert_eq!(packages.len(), 2);
        assert_eq!(packages[1].filename, "b.deb");
    }

    #[test]
    fn not_done_when_fewer_than_n() {
        let source = new_no_arch(None, false);
        let mut packages = Vec::new();
        let done = source.collect_release_packages(
            vec![release("v1.0.0", false, false, vec![asset("a.deb")])],
            &mut packages,
            5,
        );
        assert!(!done);
        assert_eq!(packages.len(), 1);
    }

    #[test]
    fn non_semver_tag_becomes_raw_version() {
        let source = new_no_arch(None, false);
        let pkgs = collect(
            &source,
            vec![release("nightly", false, false, vec![asset("n.deb")])],
            10,
        );
        assert_eq!(pkgs[0].version, PackageVersion::Raw("nightly".to_string()));
    }

    // ── arch_filter selection ──────────────────────────────────────────────

    #[test]
    fn arch_filter_prefers_amd64_over_arm64() {
        // arm64 comes first in the API response — amd64 must still win.
        let source = new_default_arch();
        let pkgs = collect(
            &source,
            vec![release(
                "v1.0.0",
                false,
                false,
                vec![asset("tool_1.0.0_arm64.deb"), asset("tool_1.0.0_amd64.deb")],
            )],
            10,
        );
        assert_eq!(pkgs.len(), 1);
        assert_eq!(pkgs[0].filename, "tool_1.0.0_amd64.deb");
    }

    #[test]
    fn arch_filter_x86_64_alias_matches_amd64_entry() {
        // An asset named with "x86_64" should match an arch_filter entry of "amd64".
        let source =
            GithubSource::new("acme", "tool", None, false, vec!["amd64".to_string()]).unwrap();
        let pkgs = collect(
            &source,
            vec![release(
                "v2.0.0",
                false,
                false,
                vec![
                    asset("tool_2.0.0_arm64.deb"),
                    asset("tool_2.0.0_x86_64.deb"),
                ],
            )],
            10,
        );
        assert_eq!(pkgs.len(), 1);
        assert_eq!(pkgs[0].filename, "tool_2.0.0_x86_64.deb");
    }

    #[test]
    fn arch_filter_respects_custom_priority_order() {
        // arm64 listed first → arm64 asset should be selected.
        let source = GithubSource::new(
            "acme",
            "tool",
            None,
            false,
            vec!["arm64".to_string(), "amd64".to_string()],
        )
        .unwrap();
        let pkgs = collect(
            &source,
            vec![release(
                "v1.0.0",
                false,
                false,
                vec![asset("tool_1.0.0_amd64.deb"), asset("tool_1.0.0_arm64.deb")],
            )],
            10,
        );
        assert_eq!(pkgs.len(), 1);
        assert_eq!(pkgs[0].filename, "tool_1.0.0_arm64.deb");
    }

    #[test]
    fn arch_filter_falls_back_to_first_asset_when_no_arch_matches() {
        // None of the assets contain a recognised arch — take the first candidate.
        let source = new_default_arch();
        let pkgs = collect(
            &source,
            vec![release(
                "v1.0.0",
                false,
                false,
                vec![
                    asset("tool_1.0.0_generic.deb"),
                    asset("tool_1.0.0_other.deb"),
                ],
            )],
            10,
        );
        assert_eq!(pkgs.len(), 1);
        assert_eq!(pkgs[0].filename, "tool_1.0.0_generic.deb");
    }

    #[test]
    fn arch_filter_with_single_asset_always_picks_it() {
        let source = new_default_arch();
        let pkgs = collect(
            &source,
            vec![release(
                "v1.0.0",
                false,
                false,
                vec![asset("tool_1.0.0_arm64.deb")],
            )],
            10,
        );
        assert_eq!(pkgs.len(), 1);
        assert_eq!(pkgs[0].filename, "tool_1.0.0_arm64.deb");
    }

    #[test]
    fn arch_filter_picks_one_per_release_across_multiple_releases() {
        let source = new_default_arch();
        let pkgs = collect(
            &source,
            vec![
                release(
                    "v2.0.0",
                    false,
                    false,
                    vec![asset("tool_2.0.0_arm64.deb"), asset("tool_2.0.0_amd64.deb")],
                ),
                release(
                    "v1.0.0",
                    false,
                    false,
                    vec![asset("tool_1.0.0_arm64.deb"), asset("tool_1.0.0_amd64.deb")],
                ),
            ],
            10,
        );
        assert_eq!(pkgs.len(), 2);
        assert_eq!(pkgs[0].filename, "tool_2.0.0_amd64.deb");
        assert_eq!(pkgs[1].filename, "tool_1.0.0_amd64.deb");
    }

    #[test]
    fn arch_filter_aarch64_alias_matches_arm64_entry() {
        // An asset named with "aarch64" should match an arch_filter entry of "arm64".
        let source = GithubSource::new(
            "acme",
            "tool",
            None,
            false,
            vec!["amd64".to_string(), "arm64".to_string()],
        )
        .unwrap();
        let pkgs = collect(
            &source,
            vec![release(
                "v3.0.0",
                false,
                false,
                vec![
                    asset("tool_3.0.0_aarch64.deb"),
                    asset("tool_3.0.0_amd64.deb"),
                ],
            )],
            10,
        );
        assert_eq!(pkgs.len(), 1);
        assert_eq!(pkgs[0].filename, "tool_3.0.0_amd64.deb");
    }

    // ── fetch_latest over a mock API server ────────────────────────────────

    use crate::test_util::{MockResponse, MockServer};

    #[tokio::test]
    async fn fetch_latest_paginates_until_empty_page() {
        let page1 = r#"[{"tag_name":"v1.0.0","prerelease":false,"draft":false,
            "assets":[{"name":"tool.deb","browser_download_url":"https://x/tool.deb"}]}]"#;
        let server = MockServer::start(vec![
            MockResponse::json(200, page1),
            MockResponse::json(200, "[]"), // second page empty → stop
        ]);
        let source = GithubSource::new("acme", "tool", None, false, vec![])
            .unwrap()
            .with_api_base(&server.url);

        let pkgs = source.fetch_latest(10).await.unwrap();
        assert_eq!(pkgs.len(), 1);
        assert_eq!(pkgs[0].filename, "tool.deb");

        let requests = server.requests();
        assert!(requests[0].starts_with("GET /repos/acme/tool/releases?page=1"));
        assert!(requests[1].starts_with("GET /repos/acme/tool/releases?page=2"));
    }

    #[tokio::test]
    async fn fetch_latest_stops_once_n_collected() {
        let page1 = r#"[{"tag_name":"v1.0.0","prerelease":false,"draft":false,
            "assets":[{"name":"tool.deb","browser_download_url":"https://x/tool.deb"}]}]"#;
        // Only one response: reaching n on page 1 must not request page 2.
        let server = MockServer::start(vec![MockResponse::json(200, page1)]);
        let source = GithubSource::new("acme", "tool", None, false, vec![])
            .unwrap()
            .with_api_base(&server.url);

        let pkgs = source.fetch_latest(1).await.unwrap();
        assert_eq!(pkgs.len(), 1);
        assert_eq!(server.requests().len(), 1);
    }

    #[tokio::test]
    async fn fetch_latest_api_error_fails() {
        let server = MockServer::start(vec![MockResponse::json(500, "{}")]);
        let source = GithubSource::new("acme", "tool", None, false, vec![])
            .unwrap()
            .with_api_base(&server.url);

        let err = source.fetch_latest(1).await.unwrap_err();
        assert!(err.to_string().contains("GitHub API error"));
    }

    #[tokio::test]
    async fn fetch_latest_invalid_json_fails() {
        let server = MockServer::start(vec![MockResponse::json(200, "not-json")]);
        let source = GithubSource::new("acme", "tool", None, false, vec![])
            .unwrap()
            .with_api_base(&server.url);

        let err = source.fetch_latest(1).await.unwrap_err();
        assert!(err.to_string().contains("Failed to parse GitHub releases"));
    }
}
