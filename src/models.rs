use serde::{Deserialize, Serialize};
use std::cmp::Ordering;
use std::fmt;

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
            // Non-semver strings (e.g. "2026.11.91422~latest", "1.2", "5:29.9.1-1")
            // use Debian version ordering so numeric segments compare
            // numerically. Fall back to the raw string as a tie-breaker to keep
            // Ord consistent with the derived Eq.
            _ => {
                let (a, b) = (self.to_string(), other.to_string());
                compare_debian_versions(&a, &b).then_with(|| a.cmp(&b))
            }
        }
    }
}

/// Compare two version strings using the dpkg algorithm
/// (`[epoch:]upstream[-revision]`, `~` sorts before everything).
pub fn compare_debian_versions(a: &str, b: &str) -> Ordering {
    fn split(v: &str) -> (u64, &str, &str) {
        let (epoch, rest) = match v.split_once(':') {
            Some((e, r)) if !e.is_empty() && e.bytes().all(|c| c.is_ascii_digit()) => {
                (e.parse().unwrap_or(0), r)
            }
            _ => (0, v),
        };
        match rest.rsplit_once('-') {
            Some((up, rev)) => (epoch, up, rev),
            None => (epoch, rest, ""),
        }
    }

    fn order(c: Option<u8>) -> i32 {
        match c {
            None => 0,
            Some(b'~') => -1,
            Some(c) if c.is_ascii_digit() => 0,
            Some(c) if c.is_ascii_alphabetic() => c as i32,
            Some(c) => c as i32 + 256,
        }
    }

    fn verrevcmp(a: &str, b: &str) -> Ordering {
        let (a, b) = (a.as_bytes(), b.as_bytes());
        let (mut i, mut j) = (0, 0);
        while i < a.len() || j < b.len() {
            while (i < a.len() && !a[i].is_ascii_digit()) || (j < b.len() && !b[j].is_ascii_digit())
            {
                let ac = order(a.get(i).copied().filter(|c| !c.is_ascii_digit()));
                let bc = order(b.get(j).copied().filter(|c| !c.is_ascii_digit()));
                if ac != bc {
                    return ac.cmp(&bc);
                }
                if i < a.len() && !a[i].is_ascii_digit() {
                    i += 1;
                }
                if j < b.len() && !b[j].is_ascii_digit() {
                    j += 1;
                }
            }
            while i < a.len() && a[i] == b'0' {
                i += 1;
            }
            while j < b.len() && b[j] == b'0' {
                j += 1;
            }
            let si = i;
            let sj = j;
            while i < a.len() && a[i].is_ascii_digit() {
                i += 1;
            }
            while j < b.len() && b[j].is_ascii_digit() {
                j += 1;
            }
            let (da, db) = (&a[si..i], &b[sj..j]);
            let ord = da.len().cmp(&db.len()).then_with(|| da.cmp(db));
            if ord != Ordering::Equal {
                return ord;
            }
        }
        Ordering::Equal
    }

    let (ea, ua, ra) = split(a);
    let (eb, ub, rb) = split(b);
    ea.cmp(&eb)
        .then_with(|| verrevcmp(ua, ub))
        .then_with(|| verrevcmp(ra, rb))
}

/// A package available from an upstream source.
#[derive(Debug, Clone)]
pub struct RemotePackage {
    pub filename: String,
    pub version: PackageVersion,
    pub download_url: String,
    pub sha256: Option<String>,
    pub package_name: Option<String>,
    pub architecture: Option<String>,
}

/// A package stored in the OpenRepo repository.
#[derive(Debug, Clone)]
pub struct RepoPackage {
    pub package_uid: String,
    pub filename: String,
    pub package_name: String,
    pub architecture: String,
    pub version: PackageVersion,
}

#[derive(Debug, Clone)]
pub enum SyncAction {
    UpToDate,
    Uploaded { version: PackageVersion },
    Skipped { version: PackageVersion },
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
        assert_eq!(
            PackageVersion::parse("1.2.3"),
            PackageVersion::parse("v1.2.3")
        );
    }

    #[test]
    fn raw_ordering_is_numeric_per_segment() {
        let a = PackageVersion::Raw("1.2".to_string());
        let b = PackageVersion::Raw("1.10".to_string());
        assert!(b > a);
    }

    #[test]
    fn debian_tilde_versions_order_numerically() {
        // Regression: Lens publishes "YYYY.M.BUILD~latest"; string comparison
        // ranked 2026.9 above 2026.11 and pruning deleted the newest upload.
        let old = PackageVersion::parse("2026.9.20601~latest");
        let new = PackageVersion::parse("2026.11.91422~latest");
        assert!(matches!(new, PackageVersion::Raw(_)));
        assert!(new > old);
    }

    #[test]
    fn debian_version_rules() {
        use std::cmp::Ordering::*;
        let cases = [
            ("1.0~rc1", "1.0", Less),
            ("1.0", "1.0+b1", Less),
            ("1:1.0", "2.0", Greater),
            (
                "5:29.9.2-1~debian.13~trixie",
                "5:29.9.10-1~debian.13~trixie",
                Less,
            ),
            ("2.4.2-3~debian.13", "2.4.2-10~debian.13", Less),
            ("1.0a", "1.0", Greater),
            ("1.01", "1.1", Equal),
            (
                "3.0.0-alpha-4-gha275~0bcc75d",
                "3.0.0-beta-2-gha280~e110527",
                Less,
            ),
        ];
        for (a, b, want) in cases {
            assert_eq!(compare_debian_versions(a, b), want, "{a} vs {b}");
        }
    }

    #[test]
    fn mixed_semver_and_raw_compare_numerically() {
        let semver = PackageVersion::parse("1.9.0");
        let raw = PackageVersion::Raw("1.10".to_string());
        assert!(raw > semver);
    }

    // ── Sort descending (mirrors pruning logic in sync.rs) ─────────────────

    #[test]
    fn sort_descending_gives_newest_first() {
        let mut versions = [
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
