use anyhow::{Context, Result};
use serde::Deserialize;
use tracing::debug;

use crate::arch::filter_by_filename_arch_priority;
use crate::models::{PackageVersion, RemotePackage};
use crate::version::{extract_architecture_from_deb_filename, extract_package_name_from_filename};

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
    /// Exact package name(s) to sync. Empty means accept all.
    pub package_filter: Vec<String>,
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
        package_filter: Vec<String>,
        client: reqwest::Client,
    ) -> Result<Self> {
        let pattern = asset_filter
            .map(|f| glob::Pattern::new(f).context("Invalid asset_filter glob pattern"))
            .transpose()?;
        Ok(Self {
            owner: owner.to_string(),
            repo: repo.to_string(),
            asset_filter: pattern,
            prerelease,
            arch_filter,
            package_filter,
            api_base: "https://api.github.com".to_string(),
            client,
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
    /// and (unless enabled) prereleases.  The parameter `n` is the number of
    /// *releases* (versions) to collect — not individual assets.  Returns true
    /// once assets from `n` releases have been collected and pagination can
    /// stop.
    fn collect_release_packages(
        &self,
        releases: Vec<Release>,
        packages: &mut Vec<RemotePackage>,
        n: usize,
    ) -> bool {
        let mut versions_collected: std::collections::HashSet<String> =
            packages.iter().map(|pkg| pkg.version.to_string()).collect();

        for release in releases {
            if release.draft {
                continue;
            }
            if release.prerelease && !self.prerelease {
                continue;
            }
            let version = PackageVersion::parse(&release.tag_name);

            // 1. Apply asset_filter (glob on filename).
            let candidates: Vec<&ReleaseAsset> = release
                .assets
                .iter()
                .filter(|a| {
                    self.asset_filter
                        .as_ref()
                        .is_none_or(|p| p.matches(&a.name))
                })
                .collect();

            // 2. Apply arch_filter: keep ALL assets matching the best
            //    (lowest-index) architecture.  When no asset matches any
            //    configured arch, all candidates are kept as a fallback so
            //    no release is silently dropped.
            let selected =
                filter_by_filename_arch_priority(candidates, &self.arch_filter, |asset| {
                    &asset.name
                });

            // 3. Apply package_filter (exact package-name match).
            let selected: Vec<&ReleaseAsset> = if self.package_filter.is_empty() {
                selected
            } else {
                selected
                    .into_iter()
                    .filter(|a| {
                        extract_package_name_from_filename(&a.name)
                            .is_some_and(|name| self.package_filter.iter().any(|f| f == &name))
                    })
                    .collect()
            };

            if selected.is_empty() {
                continue;
            }

            versions_collected.insert(version.to_string());

            for asset in selected {
                packages.push(RemotePackage {
                    filename: asset.name.clone(),
                    version: version.clone(),
                    download_url: asset.browser_download_url.clone(),
                    sha256: None,
                    package_name: extract_package_name_from_filename(&asset.name),
                    architecture: extract_architecture_from_deb_filename(&asset.name),
                });
            }

            if versions_collected.len() >= n {
                return true;
            }
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::test_client;

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
        GithubSource::new(
            "acme",
            "tool",
            asset_filter,
            prerelease,
            vec![],
            vec![],
            test_client(),
        )
        .unwrap()
    }

    fn new_default_arch() -> GithubSource {
        GithubSource::new(
            "acme",
            "tool",
            None,
            false,
            vec!["amd64".to_string(), "arm64".to_string()],
            vec![],
            test_client(),
        )
        .unwrap()
    }

    #[test]
    fn invalid_asset_filter_is_rejected() {
        let err = GithubSource::new(
            "acme",
            "tool",
            Some("[bad"),
            false,
            vec![],
            vec![],
            test_client(),
        )
        .unwrap_err();
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

    // n counts releases (versions), not individual assets.
    #[test]
    fn stops_after_n_releases() {
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
        // Both releases collected — 3 total assets (2 + 1)
        assert_eq!(packages.len(), 3);
        assert_eq!(packages[0].filename, "a.deb");
        assert_eq!(packages[1].filename, "b.deb");
        assert_eq!(packages[2].filename, "c.deb");
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
        let source = GithubSource::new(
            "acme",
            "tool",
            None,
            false,
            vec!["amd64".to_string()],
            vec![],
            test_client(),
        )
        .unwrap();
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
            vec![],
            test_client(),
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
    fn arch_filter_falls_back_to_all_candidates_when_no_arch_matches() {
        // None of the assets contain a recognised arch — keep all candidates
        // so no release is silently dropped.
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
        assert_eq!(pkgs.len(), 2);
        assert_eq!(pkgs[0].filename, "tool_1.0.0_generic.deb");
        assert_eq!(pkgs[1].filename, "tool_1.0.0_other.deb");
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
            vec![],
            test_client(),
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
        let source = GithubSource::new("acme", "tool", None, false, vec![], vec![], test_client())
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
        let source = GithubSource::new("acme", "tool", None, false, vec![], vec![], test_client())
            .unwrap()
            .with_api_base(&server.url);

        let pkgs = source.fetch_latest(1).await.unwrap();
        assert_eq!(pkgs.len(), 1);
        assert_eq!(server.requests().len(), 1);
    }

    #[tokio::test]
    async fn fetch_latest_counts_releases_across_pages() {
        let page1 = r#"[{"tag_name":"v2.0.0","prerelease":false,"draft":false,
            "assets":[
                {"name":"tool-a_2.0.0_amd64.deb","browser_download_url":"https://x/a.deb"},
                {"name":"tool-b_2.0.0_amd64.deb","browser_download_url":"https://x/b.deb"}
            ]}]"#;
        let page2 = r#"[{"tag_name":"v1.0.0","prerelease":false,"draft":false,
            "assets":[
                {"name":"tool-a_1.0.0_amd64.deb","browser_download_url":"https://x/a1.deb"},
                {"name":"tool-b_1.0.0_amd64.deb","browser_download_url":"https://x/b1.deb"}
            ]}]"#;
        let server = MockServer::start(vec![
            MockResponse::json(200, page1),
            MockResponse::json(200, page2),
        ]);
        let source = GithubSource::new("acme", "tool", None, false, vec![], vec![], test_client())
            .unwrap()
            .with_api_base(&server.url);

        let pkgs = source.fetch_latest(2).await.unwrap();

        assert_eq!(pkgs.len(), 4);
        assert_eq!(server.requests().len(), 2);
    }

    #[tokio::test]
    async fn fetch_latest_api_error_fails() {
        let server = MockServer::start(vec![MockResponse::json(500, "{}")]);
        let source = GithubSource::new("acme", "tool", None, false, vec![], vec![], test_client())
            .unwrap()
            .with_api_base(&server.url);

        let err = source.fetch_latest(1).await.unwrap_err();
        assert!(err.to_string().contains("GitHub API error"));
    }

    #[tokio::test]
    async fn fetch_latest_invalid_json_fails() {
        let server = MockServer::start(vec![MockResponse::json(200, "not-json")]);
        let source = GithubSource::new("acme", "tool", None, false, vec![], vec![], test_client())
            .unwrap()
            .with_api_base(&server.url);

        let err = source.fetch_latest(1).await.unwrap_err();
        assert!(err.to_string().contains("Failed to parse GitHub releases"));
    }

    // ── multi-package + package_filter ─────────────────────────────────────

    #[test]
    fn arch_filter_selects_all_assets_at_best_priority() {
        // Simulates rpi-imager: two different amd64 packages + two arm64 packages.
        // arch_filter [amd64, arm64] should keep BOTH amd64 packages.
        let source = new_default_arch();
        let pkgs = collect(
            &source,
            vec![release(
                "v2.0.11",
                false,
                false,
                vec![
                    asset("rpi-imager-cli_2.0.11-1_amd64.deb"),
                    asset("rpi-imager-cli_2.0.11-1_arm64.deb"),
                    asset("rpi-imager_2.0.11-1_amd64.deb"),
                    asset("rpi-imager_2.0.11-1_arm64.deb"),
                ],
            )],
            10,
        );
        assert_eq!(pkgs.len(), 2);
        assert_eq!(pkgs[0].filename, "rpi-imager-cli_2.0.11-1_amd64.deb");
        assert_eq!(pkgs[1].filename, "rpi-imager_2.0.11-1_amd64.deb");
    }

    #[test]
    fn package_filter_exact_name_match() {
        let source = GithubSource::new(
            "acme",
            "tool",
            Some("*.deb"),
            false,
            vec!["amd64".to_string()],
            vec!["rpi-imager".to_string(), "rpi-imager-cli".to_string()],
            test_client(),
        )
        .unwrap();
        let pkgs = collect(
            &source,
            vec![release(
                "v2.0.11",
                false,
                false,
                vec![
                    asset("rpi-imager-cli_2.0.11-1_amd64.deb"),
                    asset("rpi-imager-embedded_2.0.11-1_amd64.deb"),
                    asset("rpi-imager_2.0.11-1_amd64.deb"),
                ],
            )],
            10,
        );
        assert_eq!(pkgs.len(), 2);
        assert_eq!(pkgs[0].filename, "rpi-imager-cli_2.0.11-1_amd64.deb");
        assert_eq!(pkgs[1].filename, "rpi-imager_2.0.11-1_amd64.deb");
    }

    #[test]
    fn package_filter_empty_accepts_all() {
        let source = GithubSource::new(
            "acme",
            "tool",
            Some("*.deb"),
            false,
            vec!["amd64".to_string()],
            vec![],
            test_client(),
        )
        .unwrap();
        let pkgs = collect(
            &source,
            vec![release(
                "v1.0.0",
                false,
                false,
                vec![
                    asset("tool-a_1.0.0_amd64.deb"),
                    asset("tool-b_1.0.0_amd64.deb"),
                ],
            )],
            10,
        );
        assert_eq!(pkgs.len(), 2);
    }

    #[test]
    fn package_filter_combined_with_arch_and_asset_filter() {
        // Full pipeline: asset_filter → arch_filter → package_filter
        let source = GithubSource::new(
            "rpi",
            "imager",
            Some("*.deb"),
            false,
            vec!["amd64".to_string(), "arm64".to_string()],
            vec!["rpi-imager".to_string(), "rpi-imager-cli".to_string()],
            test_client(),
        )
        .unwrap();
        let pkgs = collect(
            &source,
            vec![release(
                "v2.0.11",
                false,
                false,
                vec![
                    // Non-.deb filtered by asset_filter
                    asset("imager-v2.0.11.exe"),
                    asset("rpi-imager-v2.0.11.dmg"),
                    // arm64 filtered by arch_filter (amd64 wins)
                    asset("rpi-imager-cli_2.0.11-1_arm64.deb"),
                    // Passes all filters
                    asset("rpi-imager-cli_2.0.11-1_amd64.deb"),
                    asset("rpi-imager_2.0.11-1_amd64.deb"),
                    // Filtered by package_filter
                    asset("rpi-imager-embedded_2.0.11-1_amd64.deb"),
                ],
            )],
            10,
        );
        assert_eq!(pkgs.len(), 2);
        assert_eq!(pkgs[0].filename, "rpi-imager-cli_2.0.11-1_amd64.deb");
        assert_eq!(pkgs[1].filename, "rpi-imager_2.0.11-1_amd64.deb");
    }

    #[test]
    fn package_name_and_architecture_populated() {
        let source = new_no_arch(None, false);
        let pkgs = collect(
            &source,
            vec![release(
                "v1.0.0",
                false,
                false,
                vec![asset("rpi-imager-cli_1.0.0-1_amd64.deb")],
            )],
            10,
        );
        assert_eq!(pkgs[0].package_name.as_deref(), Some("rpi-imager-cli"));
        assert_eq!(pkgs[0].architecture.as_deref(), Some("amd64"));
    }

    #[test]
    fn counts_releases_not_individual_assets() {
        // With n=3 and 2 packages per release, we should get 3 releases (6 assets),
        // not stop after 3 individual assets.
        let source = new_default_arch();
        let mut packages = Vec::new();
        let done = source.collect_release_packages(
            vec![
                release(
                    "v3.0.0",
                    false,
                    false,
                    vec![
                        asset("tool-a_3.0.0_amd64.deb"),
                        asset("tool-b_3.0.0_amd64.deb"),
                    ],
                ),
                release(
                    "v2.0.0",
                    false,
                    false,
                    vec![
                        asset("tool-a_2.0.0_amd64.deb"),
                        asset("tool-b_2.0.0_amd64.deb"),
                    ],
                ),
                release(
                    "v1.0.0",
                    false,
                    false,
                    vec![
                        asset("tool-a_1.0.0_amd64.deb"),
                        asset("tool-b_1.0.0_amd64.deb"),
                    ],
                ),
            ],
            &mut packages,
            3,
        );
        assert!(done);
        assert_eq!(packages.len(), 6);
    }
}
