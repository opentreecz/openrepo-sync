use anyhow::{Context, Result, bail};
use reqwest::{
    Client, StatusCode,
    multipart::{Form, Part},
};
use serde::Deserialize;
use std::path::Path;
use tracing::{debug, warn};

use crate::models::{PackageVersion, RepoPackage};
use crate::version::extract_version_from_filename;

pub struct RepoClient {
    base_url: String,
    api_key: String,
    client: Client,
}

#[derive(Debug, Deserialize)]
struct UserResponse {
    username: String,
}

#[allow(dead_code)]
struct PackageListResponse {
    results: Vec<PackageEntry>,
    next: Option<String>,
}

#[allow(dead_code)]
struct PackageEntry {
    package_uid: String,
    package_name: String,
}

impl RepoClient {
    pub fn new(base_url: &str, api_key: &str) -> Result<Self> {
        let client = Client::builder()
            .user_agent("openrepo-sync/0.1")
            .build()
            .context("Failed to create HTTP client")?;
        Ok(Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            api_key: api_key.to_string(),
            client,
        })
    }

    fn auth_header(&self) -> String {
        format!("Token {}", self.api_key)
    }

    pub async fn whoami(&self) -> Result<String> {
        let url = format!("{}/api/whoami", self.base_url);
        let resp = self
            .client
            .get(&url)
            .header("Authorization", self.auth_header())
            .send()
            .await
            .context("Failed to reach OpenRepo server")?;
        if resp.status() == StatusCode::UNAUTHORIZED {
            bail!("OpenRepo authentication failed — check your API key");
        }
        if !resp.status().is_success() {
            bail!("whoami request failed: {}", resp.status());
        }
        let user: UserResponse = resp
            .json()
            .await
            .context("Failed to parse whoami response")?;
        Ok(user.username)
    }

    pub async fn list_packages(&self, repo_uid: &str) -> Result<Vec<RepoPackage>> {
        let mut packages = Vec::new();
        let mut page_url = Some(format!(
            "{}/api/repos/{}/packages/",
            self.base_url, repo_uid
        ));

        while let Some(u) = page_url.take() {
            debug!("Fetching packages from {}", u);
            let resp = self
                .client
                .get(&u)
                .header("Authorization", self.auth_header())
                .send()
                .await
                .context("Failed to list packages")?;

            if resp.status() == StatusCode::NOT_FOUND {
                debug!(
                    "Packages endpoint returned 404 for repo '{}' — repo may be empty or endpoint path differs",
                    repo_uid
                );
                break;
            }

            if !resp.status().is_success() {
                bail!(
                    "Failed to list packages for {}: {}",
                    repo_uid,
                    resp.status()
                );
            }

            let body: serde_json::Value = resp
                .json()
                .await
                .context("Failed to parse package list response")?;

            match body.get("results").and_then(|r| r.as_array()) {
                Some(results) => {
                    debug!("Got {} packages in this page", results.len());
                    for pkg in results {
                        let package_uid = pkg
                            .get("package_uid")
                            .and_then(|v| v.as_str())
                            .unwrap_or_default()
                            .to_string();
                        let filename = pkg
                            .get("package_name")
                            .and_then(|v| v.as_str())
                            .unwrap_or_default()
                            .to_string();
                        let version = extract_version_from_filename(&filename)
                            .unwrap_or(PackageVersion::Raw("0".to_string()));
                        packages.push(RepoPackage {
                            package_uid,
                            filename,
                            version,
                        });
                    }
                    page_url = body
                        .get("next")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string());
                }
                None => {
                    warn!(
                        "Unexpected response format from packages endpoint for '{}' — \
                         expected {{\"results\": [...]}}, got: {}",
                        repo_uid,
                        body.to_string().chars().take(200).collect::<String>()
                    );
                    break;
                }
            }
        }

        Ok(packages)
    }

    pub async fn upload_package(&self, repo_uid: &str, path: &Path, overwrite: bool) -> Result<()> {
        let url = format!("{}/api/{}/upload/", self.base_url, repo_uid);
        let filename = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("package")
            .to_string();

        debug!("Uploading {} to {}", filename, url);

        let bytes = tokio::fs::read(path)
            .await
            .with_context(|| format!("Failed to read file for upload: {}", path.display()))?;

        let file_part = Part::bytes(bytes).file_name(filename);
        let mut form = Form::new().part("package_file", file_part);
        if overwrite {
            form = form.text("overwrite", "1");
        }

        let resp = self
            .client
            .post(&url)
            .header("Authorization", self.auth_header())
            .multipart(form)
            .send()
            .await
            .context("Upload request failed")?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            bail!("Upload failed ({}): {}", status, body);
        }
        Ok(())
    }

    pub async fn delete_package(&self, repo_uid: &str, package_uid: &str) -> Result<()> {
        let url = format!("{}/api/{}/pkg/{}/", self.base_url, repo_uid, package_uid);
        debug!("Deleting package {} from repo {}", package_uid, repo_uid);
        let resp = self
            .client
            .delete(&url)
            .header("Authorization", self.auth_header())
            .send()
            .await
            .context("Delete request failed")?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            bail!("Delete failed ({}): {}", status, body);
        }
        Ok(())
    }
}
