use std::cmp::Ordering;
use std::fmt;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum PackageVersion {
    Semver(semver::Version),
    Raw(String),
}

impl PackageVersion {
    pub fn parse(s: &str) -> Self {
        let cleaned = s.trim_start_matches('v');
        match semver::Version::parse(cleaned) {
            Ok(v) => PackageVersion::Semver(v),
            Err(_) => PackageVersion::Raw(s.to_string()),
        }
    }
}

impl fmt::Display for PackageVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PackageVersion::Semver(v) => write!(f, "{}", v),
            PackageVersion::Raw(s) => write!(f, "{}", s),
        }
    }
}

impl PartialOrd for PackageVersion {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for PackageVersion {
    fn cmp(&self, other: &Self) -> Ordering {
        match (self, other) {
            (PackageVersion::Semver(a), PackageVersion::Semver(b)) => a.cmp(b),
            _ => self.to_string().cmp(&other.to_string()),
        }
    }
}

/// A package available from an upstream source.
#[derive(Debug, Clone)]
pub struct RemotePackage {
    pub filename: String,
    pub version: PackageVersion,
    pub download_url: String,
}

/// A package stored in the OpenRepo repository.
#[derive(Debug, Clone)]
pub struct RepoPackage {
    pub package_uid: String,
    pub filename: String,
    pub version: PackageVersion,
}

#[derive(Debug, Clone)]
pub enum SyncAction {
    UpToDate,
    Uploaded { version: PackageVersion },
    Pruned { removed_count: usize },
    Error(String),
}

#[derive(Debug, Clone)]
pub struct SyncResult {
    pub project_name: String,
    pub actions: Vec<SyncAction>,
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── PackageVersion::parse ──────────────────────────────────────────────

    #[test]
    fn parse_plain_semver() {
        assert_eq!(
            PackageVersion::parse("1.2.3"),
            PackageVersion::Semver("1.2.3".parse().unwrap())
        );
    }

    #[test]
    fn parse_strips_v_prefix() {
        assert_eq!(
            PackageVersion::parse("v1.2.3"),
            PackageVersion::Semver("1.2.3".parse().unwrap())
        );
    }

    #[test]
    fn parse_semver_with_prerelease() {
        assert_eq!(
            PackageVersion::parse("1.2.3-rc1"),
            PackageVersion::Semver("1.2.3-rc1".parse().unwrap())
        );
    }

    #[test]
    fn parse_non_semver_falls_back_to_raw() {
        assert_eq!(
            PackageVersion::parse("bookworm"),
            PackageVersion::Raw("bookworm".to_string())
        );
    }

    #[test]
    fn parse_two_part_version_is_raw() {
        // "1.2" is not valid semver (requires three components)
        assert_eq!(
            PackageVersion::parse("1.2"),
            PackageVersion::Raw("1.2".to_string())
        );
    }

    // ── PackageVersion::Display ────────────────────────────────────────────

    #[test]
    fn display_semver() {
        assert_eq!(PackageVersion::parse("v2.0.0").to_string(), "2.0.0");
    }

    #[test]
    fn display_raw() {
        assert_eq!(PackageVersion::parse("nightly").to_string(), "nightly");
    }

    // ── Ordering ──────────────────────────────────────────────────────────

    #[test]
    fn semver_ordering() {
        let v1 = PackageVersion::parse("1.0.0");
        let v2 = PackageVersion::parse("2.0.0");
        let v3 = PackageVersion::parse("1.9.9");
        assert!(v2 > v1);
        assert!(v3 > v1);
        assert!(v2 > v3);
    }

    #[test]
    fn semver_prerelease_less_than_release() {
        // semver spec: pre-release version has lower precedence than release
        let rc = PackageVersion::parse("1.0.0-rc1");
        let release = PackageVersion::parse("1.0.0");
        assert!(rc < release);
    }

    #[test]
    fn equal_versions_compare_equal() {
        assert_eq!(PackageVersion::parse("1.2.3"), PackageVersion::parse("v1.2.3"));
    }

    #[test]
    fn raw_ordering_is_lexicographic() {
        let a = PackageVersion::Raw("1.2".to_string());
        let b = PackageVersion::Raw("1.10".to_string());
        // lexicographic: "1.10" < "1.2" because '1' < '2'
        assert!(b < a);
    }

    // ── Sort descending (mirrors pruning logic in sync.rs) ─────────────────

    #[test]
    fn sort_descending_gives_newest_first() {
        let mut versions = vec![
            PackageVersion::parse("1.0.0"),
            PackageVersion::parse("3.0.0"),
            PackageVersion::parse("2.0.0"),
        ];
        versions.sort_by(|a, b| b.cmp(a));
        assert_eq!(versions[0], PackageVersion::parse("3.0.0"));
        assert_eq!(versions[1], PackageVersion::parse("2.0.0"));
        assert_eq!(versions[2], PackageVersion::parse("1.0.0"));
    }
}
