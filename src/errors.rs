// Copyright 2026 openrepo-sync contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Typed error types for OpenRepo API responses.
//!
//! The server returns a structured error envelope for all failures:
//!
//! ```json
//! {"code": "PACKAGE_EXISTS", "detail": "...", "status": 409}
//! ```
//!
//! [`ApiError`] deserialises that envelope.  [`UploadError`] is the typed
//! error returned by [`crate::repo_client::RepoClient::upload_package`] so
//! that callers can branch on specific failure modes without string matching.

use serde::Deserialize;
use thiserror::Error;

/// Deserialised form of the server's structured error envelope.
#[allow(dead_code)]
///
/// Present on HTTP 4xx/5xx responses from OpenRepo ≥ 2.5.0.
/// Older servers may return plain text or a different shape; callers should
/// treat a parse failure as an unknown error.
#[derive(Debug, Deserialize)]
pub struct ApiError {
    /// Machine-readable error code (e.g. `"PACKAGE_EXISTS"`).
    pub code: String,
    /// Human-readable description.
    pub detail: String,
    /// HTTP status code echoed in the body.
    #[serde(default)]
    pub status: u16,
}

/// Known server-side error codes returned in [`ApiError::code`].
#[allow(dead_code)]
pub mod code {
    pub const PACKAGE_EXISTS: &str = "PACKAGE_EXISTS";
    pub const REPO_NOT_FOUND: &str = "REPO_NOT_FOUND";
    pub const KEY_IN_USE: &str = "KEY_IN_USE";
    pub const VALIDATION_ERROR: &str = "VALIDATION_ERROR";
    pub const AUTHENTICATION_FAILED: &str = "AUTHENTICATION_FAILED";
    pub const PERMISSION_DENIED: &str = "PERMISSION_DENIED";
    pub const NOT_FOUND: &str = "NOT_FOUND";
}

/// Typed error returned by
/// [`RepoClient::upload_package`](crate::repo_client::RepoClient::upload_package).
///
/// Separating `PackageExists` from the catch-all `Other` lets callers handle
/// the conflict case without fragile string matching.
#[derive(Debug, Error)]
pub enum UploadError {
    /// The package already exists in the repository and `overwrite` was not
    /// requested.  Maps to HTTP 409 with `code == "PACKAGE_EXISTS"`, or to a
    /// failed async task with `error_code == "PACKAGE_EXISTS"`.
    #[error("package already exists in repository")]
    PackageExists,

    /// Any other upload failure (network error, server error, auth failure,
    /// …).
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn api_error_deserialises_full_envelope() {
        let json = r#"{"code":"PACKAGE_EXISTS","detail":"already there","status":409}"#;
        let err: ApiError = serde_json::from_str(json).unwrap();
        assert_eq!(err.code, "PACKAGE_EXISTS");
        assert_eq!(err.detail, "already there");
        assert_eq!(err.status, 409);
    }

    #[test]
    fn api_error_status_defaults_to_zero_when_absent() {
        let json = r#"{"code":"NOT_FOUND","detail":"gone"}"#;
        let err: ApiError = serde_json::from_str(json).unwrap();
        assert_eq!(err.status, 0);
    }

    #[test]
    fn upload_error_package_exists_display() {
        let e = UploadError::PackageExists;
        assert_eq!(e.to_string(), "package already exists in repository");
    }

    #[test]
    fn upload_error_other_is_transparent() {
        let inner = anyhow::anyhow!("network timeout");
        let e = UploadError::Other(inner);
        assert!(e.to_string().contains("network timeout"));
    }
}
