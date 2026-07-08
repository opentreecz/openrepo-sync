use crate::models::PackageVersion;
use anyhow::{Context, Result, bail};
use std::path::Path;

pub fn extract_version_from_filename(filename: &str) -> Option<PackageVersion> {
    // Strip known package/archive extensions before matching so the extension
    // is never captured as part of the version string.
    let stripped = filename
        .trim_end_matches(".deb")
        .trim_end_matches(".rpm")
        .trim_end_matches(".tar.gz")
        .trim_end_matches(".tar.bz2")
        .trim_end_matches(".tar.xz")
        .trim_end_matches(".zip")
        .trim_end_matches(".tgz");

    // Match semver-like versions including pre-release/build metadata:
    //   name-1.2.3, name_1.2.3_amd64, name-v1.2.3-rc1
    let re = regex::Regex::new(r"[-_]v?(\d+\.\d+[\d.\-+a-zA-Z]*)").unwrap();
    re.captures(stripped)
        .and_then(|c| c.get(1))
        .map(|m| PackageVersion::parse(m.as_str()))
}

pub fn extract_version_dpkg(path: &Path) -> Result<String> {
    let output = std::process::Command::new("dpkg-deb")
        .args(["--field", &path.to_string_lossy(), "Version"])
        .output()
        .context("Failed to run dpkg-deb — is dpkg installed?")?;
    if !output.status.success() {
        bail!(
            "dpkg-deb failed for {}: {}",
            path.display(),
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

pub fn extract_version_rpm(path: &Path) -> Result<String> {
    let output = std::process::Command::new("rpm")
        .args([
            "-qp",
            "--queryformat",
            "%{VERSION}-%{RELEASE}",
            &path.to_string_lossy(),
        ])
        .output()
        .context("Failed to run rpm — is rpm installed?")?;
    if !output.status.success() {
        bail!(
            "rpm query failed for {}: {}",
            path.display(),
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

pub fn extract_version_from_package(path: &Path) -> Result<PackageVersion> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or_default();
    let version_str = match ext {
        "deb" => extract_version_dpkg(path)?,
        "rpm" => extract_version_rpm(path)?,
        _ => bail!("Unsupported package type for version extraction: .{}", ext),
    };
    Ok(PackageVersion::parse(&version_str))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::PackageVersion;

    // ── extract_version_from_filename ──────────────────────────────────────

    #[test]
    fn filename_dash_semver_deb() {
        let v = extract_version_from_filename("curl-8.5.0.deb").unwrap();
        assert_eq!(v, PackageVersion::parse("8.5.0"));
    }

    #[test]
    fn filename_underscore_semver_deb() {
        let v = extract_version_from_filename("curl_8.5.0_amd64.deb").unwrap();
        assert_eq!(v, PackageVersion::parse("8.5.0"));
    }

    #[test]
    fn filename_v_prefix() {
        let v = extract_version_from_filename("mypkg-v1.2.3.tar.gz").unwrap();
        assert_eq!(v, PackageVersion::parse("1.2.3"));
    }

    #[test]
    fn filename_prerelease_suffix() {
        let v = extract_version_from_filename("mypkg-1.2.3-rc1.deb").unwrap();
        assert_eq!(v, PackageVersion::parse("1.2.3-rc1"));
    }

    #[test]
    fn filename_no_version_returns_none() {
        assert!(extract_version_from_filename("LATEST.deb").is_none());
        assert!(extract_version_from_filename("package.deb").is_none());
    }

    #[test]
    fn filename_build_metadata() {
        let v = extract_version_from_filename("tool-1.0.0+build42.deb").unwrap();
        assert_eq!(v.to_string(), "1.0.0+build42");
    }

    // ── extract_version_from_package: unsupported extension ───────────────

    #[test]
    fn unsupported_extension_returns_error() {
        let path = std::path::Path::new("somefile.zip");
        let err = extract_version_from_package(path).unwrap_err();
        assert!(err.to_string().contains("Unsupported package type"));
    }
}
