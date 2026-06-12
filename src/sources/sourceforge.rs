use anyhow::{Context, Result};
use scraper::{Html, Selector};
use tracing::debug;

use crate::models::{PackageVersion, RemotePackage};
use crate::version::extract_version_from_filename;

pub struct SourceforgeSource {
    pub project: String,
    pub folder: Option<String>,
    pub filename_filter: Option<glob::Pattern>,
    client: reqwest::Client,
}

impl SourceforgeSource {
    pub fn new(
        project: &str,
        folder: Option<&str>,
        filename_filter: Option<&str>,
    ) -> Result<Self> {
        let pattern = filename_filter
            .map(|f| glob::Pattern::new(f).context("Invalid filename_filter glob pattern"))
            .transpose()?;
        let client = reqwest::Client::builder()
            .user_agent("openrepo-sync/0.1")
            .build()?;
        Ok(Self {
            project: project.to_string(),
            folder: folder.map(|s| s.trim_matches('/').to_string()),
            filename_filter: pattern,
            client,
        })
    }

    pub async fn fetch_latest(&self, n: usize) -> Result<Vec<RemotePackage>> {
        let folder_path = self.folder.as_deref().unwrap_or("");
        let url = if folder_path.is_empty() {
            format!(
                "https://sourceforge.net/projects/{}/files/",
                self.project
            )
        } else {
            format!(
                "https://sourceforge.net/projects/{}/files/{}/",
                self.project, folder_path
            )
        };
        debug!("Fetching SourceForge listing from {}", url);

        let html = self
            .client
            .get(&url)
            .send()
            .await
            .context("Failed to fetch SourceForge page")?
            .error_for_status()
            .context("SourceForge page error")?
            .text()
            .await
            .context("Failed to read SourceForge page")?;

        let packages = self.parse_files(&html, n)?;
        Ok(packages)
    }

    fn parse_files(&self, html: &str, n: usize) -> Result<Vec<RemotePackage>> {
        let document = Html::parse_document(html);
        // SourceForge file listings use a table with id="files_list"
        let row_sel = Selector::parse("table#files_list tbody tr[title]").unwrap();
        let link_sel = Selector::parse("th.name a").unwrap();

        let mut packages = Vec::new();

        for row in document.select(&row_sel) {
            let title = row.value().attr("title").unwrap_or_default();
            // Skip directory rows (they don't have a file extension in the title)
            if !title.contains('.') {
                continue;
            }

            if let Some(link) = row.select(&link_sel).next() {
                let filename = link.text().collect::<String>().trim().to_string();
                if filename.is_empty() {
                    continue;
                }
                if let Some(pattern) = &self.filename_filter {
                    if !pattern.matches(&filename) {
                        continue;
                    }
                }
                let href = link.value().attr("href").unwrap_or_default();
                // SourceForge download links: /projects/<proj>/files/<path>/download
                let download_url = if href.starts_with('/') {
                    format!("https://sourceforge.net{}", href)
                } else {
                    href.to_string()
                };
                let version = extract_version_from_filename(&filename)
                    .unwrap_or(PackageVersion::Raw("0".to_string()));
                packages.push(RemotePackage {
                    filename,
                    version,
                    download_url,
                });
            }
            if packages.len() >= n {
                break;
            }
        }

        // Sort descending by version so newest is first
        packages.sort_by(|a, b| b.version.cmp(&a.version));
        Ok(packages.into_iter().take(n).collect())
    }
}
