//! Shared GPG signature verification for deb and rpm repository sources.
//!
//! Both `deb_repo` and `rpm_repo` need to verify GPG signatures on repository
//! metadata.  This module extracts the shared logic: checking for the `gpg`
//! binary, resolving key material (URL or inline), dearmoring the key, and
//! running `gpg --verify`.

use anyhow::{Context, Result, bail};
use std::path::Path;
use tracing::debug;

/// Check that the `gpg` binary is available on the system PATH.
pub fn ensure_gpg_available() -> Result<()> {
    if std::process::Command::new("gpg")
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_err()
    {
        bail!(
            "gpg binary not found. Install gpg or set verify_gpg: false \
             in the project config to skip signature verification"
        );
    }
    Ok(())
}

/// Resolve GPG key material from either a URL or an inline string.
pub async fn resolve_key(client: &reqwest::Client, key_source: &str) -> Result<Vec<u8>> {
    if key_source.starts_with("http://") || key_source.starts_with("https://") {
        Ok(client
            .get(key_source)
            .send()
            .await
            .context("Failed to fetch GPG key URL")?
            .error_for_status()
            .context("GPG key URL request error")?
            .bytes()
            .await
            .context("Failed to read GPG key body")?
            .to_vec())
    } else {
        Ok(key_source.as_bytes().to_vec())
    }
}

/// Dearmor a GPG key and return the path to the dearmored keyring.
///
/// Writes the raw `key_data` into `tmp_dir/repo.gpg`, runs `gpg --dearmor`,
/// and produces `tmp_dir/repo-dearmored.gpg`.
fn dearmor_key(tmp_dir: &Path, key_data: &[u8]) -> Result<std::path::PathBuf> {
    let keyring = tmp_dir.join("repo.gpg");
    let dearmored = tmp_dir.join("repo-dearmored.gpg");

    std::fs::write(&keyring, key_data).context("Failed to write GPG keyring")?;

    let output = std::process::Command::new("gpg")
        .args([
            "--homedir",
            tmp_dir.to_str().unwrap(),
            "--dearmor",
            "--output",
            dearmored.to_str().unwrap(),
            keyring.to_str().unwrap(),
        ])
        .output()
        .context("Failed to run gpg --dearmor (is gpg installed?)")?;

    if !output.status.success() {
        bail!(
            "gpg --dearmor failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    Ok(dearmored)
}

/// Verify a clearsigned file (e.g. Debian InRelease).
///
/// The `signed_data` is a clearsigned PGP message where the signature and data
/// are combined in one file.  `gpg --verify <file>` checks this directly.
pub fn verify_clearsigned(
    tmp_dir: &Path,
    key_data: &[u8],
    signed_data: &[u8],
    label: &str,
) -> Result<()> {
    let dearmored = dearmor_key(tmp_dir, key_data)?;

    let signed_file = tmp_dir.join("signed-data");
    std::fs::write(&signed_file, signed_data).context("Failed to write signed data")?;

    let output = std::process::Command::new("gpg")
        .args([
            "--homedir",
            tmp_dir.to_str().unwrap(),
            "--no-default-keyring",
            "--keyring",
            dearmored.to_str().unwrap(),
            "--verify",
            signed_file.to_str().unwrap(),
        ])
        .output()
        .context("Failed to run gpg --verify")?;

    if output.status.success() {
        debug!("GPG signature verified for {label}");
        Ok(())
    } else {
        bail!(
            "GPG signature verification failed for {label}: {}",
            String::from_utf8_lossy(&output.stderr)
        )
    }
}

/// Verify a detached signature (e.g. RPM repomd.xml + repomd.xml.asc).
///
/// `signature` is the detached `.asc` file; `data` is the file that was signed.
/// `gpg --verify <sig> <data>` checks the detached signature.
pub fn verify_detached(
    tmp_dir: &Path,
    key_data: &[u8],
    data: &[u8],
    signature: &[u8],
    label: &str,
) -> Result<()> {
    let dearmored = dearmor_key(tmp_dir, key_data)?;

    let data_file = tmp_dir.join("data");
    let sig_file = tmp_dir.join("data.asc");
    std::fs::write(&data_file, data).context("Failed to write data file")?;
    std::fs::write(&sig_file, signature).context("Failed to write signature file")?;

    let output = std::process::Command::new("gpg")
        .args([
            "--homedir",
            tmp_dir.to_str().unwrap(),
            "--no-default-keyring",
            "--keyring",
            dearmored.to_str().unwrap(),
            "--verify",
            sig_file.to_str().unwrap(),
            data_file.to_str().unwrap(),
        ])
        .output()
        .context("Failed to run gpg --verify")?;

    if output.status.success() {
        debug!("GPG signature verified for {label}");
        Ok(())
    } else {
        bail!(
            "GPG signature verification failed for {label}: {}",
            String::from_utf8_lossy(&output.stderr)
        )
    }
}
