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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::{MockResponse, MockServer};

    #[test]
    fn new_trims_trailing_slash_from_base_url() {
        let client = RepoClient::new("https://repo.example.com/", "key").unwrap();
        assert_eq!(client.base_url, "https://repo.example.com");
    }

    #[test]
    fn auth_header_uses_token_scheme() {
        let client = RepoClient::new("https://repo.example.com", "s3cret").unwrap();
        assert_eq!(client.auth_header(), "Token s3cret");
    }

    // ── whoami ─────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn whoami_returns_username_and_sends_auth() {
        let server = MockServer::start(vec![MockResponse::json(200, r#"{"username":"alice"}"#)]);
        let client = RepoClient::new(&server.url, "k1").unwrap();
        let user = client.whoami().await.unwrap();
        assert_eq!(user, "alice");

        let requests = server.requests();
        assert!(requests[0].starts_with("GET /api/whoami"));
        assert!(requests[0].contains("authorization: Token k1"));
    }

    #[tokio::test]
    async fn whoami_unauthorized_is_a_clear_error() {
        let server = MockServer::start(vec![MockResponse::json(401, "{}")]);
        let client = RepoClient::new(&server.url, "bad").unwrap();
        let err = client.whoami().await.unwrap_err();
        assert!(err.to_string().contains("authentication failed"));
    }

    #[tokio::test]
    async fn whoami_server_error_reports_status() {
        let server = MockServer::start(vec![MockResponse::json(500, "{}")]);
        let client = RepoClient::new(&server.url, "k").unwrap();
        let err = client.whoami().await.unwrap_err();
        assert!(err.to_string().contains("whoami request failed"));
    }

    #[tokio::test]
    async fn whoami_invalid_json_is_a_parse_error() {
        let server = MockServer::start(vec![MockResponse::json(200, "not-json")]);
        let client = RepoClient::new(&server.url, "k").unwrap();
        let err = client.whoami().await.unwrap_err();
        assert!(err.to_string().contains("parse"));
    }

    // ── list_packages ──────────────────────────────────────────────────────

    #[tokio::test]
    async fn list_packages_parses_results_and_extracts_versions() {
        let body = r#"{"results":[
            {"package_uid":"u1","package_name":"curl_8.5.0_amd64.deb"},
            {"package_uid":"u2","package_name":"noversion.deb"}
        ],"next":null}"#;
        let server = MockServer::start(vec![MockResponse::json(200, body)]);
        let client = RepoClient::new(&server.url, "k").unwrap();
        let pkgs = client.list_packages("myrepo").await.unwrap();

        assert_eq!(pkgs.len(), 2);
        assert_eq!(pkgs[0].package_uid, "u1");
        assert_eq!(pkgs[0].filename, "curl_8.5.0_amd64.deb");
        assert_eq!(pkgs[0].version, PackageVersion::parse("8.5.0"));
        // Unversioned filename falls back to raw "0"
        assert_eq!(pkgs[1].version, PackageVersion::Raw("0".to_string()));

        let requests = server.requests();
        assert!(requests[0].starts_with("GET /api/repos/myrepo/packages/"));
    }

    #[tokio::test]
    async fn list_packages_follows_next_page() {
        // Page 1's "next" link must point back at the live server, so the
        // response set is built from the bound URL.
        let server = MockServer::start_with(|url| {
            vec![
                MockResponse::json(
                    200,
                    &format!(
                        r#"{{"results":[{{"package_uid":"u1","package_name":"a-1.0.0.deb"}}],"next":"{}/api/repos/r/packages/?page=2"}}"#,
                        url
                    ),
                ),
                MockResponse::json(
                    200,
                    r#"{"results":[{"package_uid":"u2","package_name":"b-2.0.0.deb"}],"next":null}"#,
                ),
            ]
        });
        let client = RepoClient::new(&server.url, "k").unwrap();
        let pkgs = client.list_packages("r").await.unwrap();
        assert_eq!(pkgs.len(), 2);
        assert_eq!(pkgs[0].package_uid, "u1");
        assert_eq!(pkgs[1].package_uid, "u2");
    }

    #[tokio::test]
    async fn list_packages_404_means_empty() {
        let server = MockServer::start(vec![MockResponse::json(404, "{}")]);
        let client = RepoClient::new(&server.url, "k").unwrap();
        let pkgs = client.list_packages("missing").await.unwrap();
        assert!(pkgs.is_empty());
    }

    #[tokio::test]
    async fn list_packages_unexpected_format_yields_empty() {
        let server = MockServer::start(vec![MockResponse::json(200, r#"{"foo":"bar"}"#)]);
        let client = RepoClient::new(&server.url, "k").unwrap();
        let pkgs = client.list_packages("r").await.unwrap();
        assert!(pkgs.is_empty());
    }

    #[tokio::test]
    async fn list_packages_server_error_fails() {
        let server = MockServer::start(vec![MockResponse::json(500, "boom")]);
        let client = RepoClient::new(&server.url, "k").unwrap();
        let err = client.list_packages("r").await.unwrap_err();
        assert!(err.to_string().contains("Failed to list packages"));
    }

    // ── upload_package ─────────────────────────────────────────────────────

    #[tokio::test]
    async fn upload_package_posts_multipart_to_repo_endpoint() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tool-1.0.0.deb");
        std::fs::write(&path, b"fake-deb-bytes").unwrap();

        let server = MockServer::start(vec![MockResponse::json(200, "{}")]);
        let client = RepoClient::new(&server.url, "k").unwrap();
        client.upload_package("myrepo", &path, false).await.unwrap();

        let requests = server.requests();
        assert!(requests[0].starts_with("POST /api/myrepo/upload/"));
        assert!(requests[0].contains("name=\"package_file\""));
        assert!(requests[0].contains("tool-1.0.0.deb"));
        assert!(!requests[0].contains("name=\"overwrite\""));
    }

    #[tokio::test]
    async fn upload_package_sets_overwrite_field() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tool-1.0.0.deb");
        std::fs::write(&path, b"fake").unwrap();

        let server = MockServer::start(vec![MockResponse::json(200, "{}")]);
        let client = RepoClient::new(&server.url, "k").unwrap();
        client.upload_package("r", &path, true).await.unwrap();

        let requests = server.requests();
        assert!(requests[0].contains("name=\"overwrite\""));
    }

    #[tokio::test]
    async fn upload_package_failure_includes_status_and_body() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tool-1.0.0.deb");
        std::fs::write(&path, b"fake").unwrap();

        let server = MockServer::start(vec![MockResponse::json(400, "duplicate package")]);
        let client = RepoClient::new(&server.url, "k").unwrap();
        let err = client.upload_package("r", &path, false).await.unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("400"));
        assert!(msg.contains("duplicate package"));
    }

    #[tokio::test]
    async fn upload_package_missing_file_fails_before_request() {
        let client = RepoClient::new("http://127.0.0.1:1", "k").unwrap();
        let err = client
            .upload_package("r", Path::new("/nonexistent/file.deb"), false)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("Failed to read file for upload"));
    }

    // ── delete_package ─────────────────────────────────────────────────────

    #[tokio::test]
    async fn delete_package_hits_pkg_endpoint() {
        let server = MockServer::start(vec![MockResponse::json(200, "")]);
        let client = RepoClient::new(&server.url, "k").unwrap();
        client.delete_package("myrepo", "uid-42").await.unwrap();

        let requests = server.requests();
        assert!(requests[0].starts_with("DELETE /api/myrepo/pkg/uid-42/"));
    }

    #[tokio::test]
    async fn delete_package_failure_includes_status_and_body() {
        let server = MockServer::start(vec![MockResponse::json(403, "forbidden")]);
        let client = RepoClient::new(&server.url, "k").unwrap();
        let err = client.delete_package("r", "u").await.unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("403"));
        assert!(msg.contains("forbidden"));
    }
}
