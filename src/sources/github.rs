use anyhow::{Context, Result};
use serde::Deserialize;
use tracing::debug;

use crate::models::{PackageVersion, RemotePackage};

pub struct GithubSource {
    pub owner: String,
    pub repo: String,
    pub asset_filter: Option<glob::Pattern>,
    pub prerelease: bool,
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
            client,
        })
    }

    pub async fn fetch_latest(&self, n: usize) -> Result<Vec<RemotePackage>> {
        let base_url = format!(
            "https://api.github.com/repos/{}/{}/releases",
            self.owner, self.repo
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

            for release in releases {
                if release.draft {
                    continue;
                }
                if release.prerelease && !self.prerelease {
                    continue;
                }
                let version = PackageVersion::parse(&release.tag_name);
                for asset in &release.assets {
                    if let Some(pattern) = &self.asset_filter
                        && !pattern.matches(&asset.name)
                    {
                        continue;
                    }
                    packages.push(RemotePackage {
                        filename: asset.name.clone(),
                        version: version.clone(),
                        download_url: asset.browser_download_url.clone(),
                    });
                    if packages.len() >= n {
                        break 'outer;
                    }
                }
            }

            page += 1;
        }

        Ok(packages)
    }
}
