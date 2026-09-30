//! Shared architecture policy used by all source types.
//!
//! Filename matching is delimiter-aware so short aliases like `x86` do not
//! accidentally match `x86_64` assets.

pub const AMD64_ALIASES: &[&str] = &["amd64", "x86_64", "x86-64", "x64"];
pub const ARM64_ALIASES: &[&str] = &["arm64", "aarch64", "arm-64"];
pub const ARMHF_ALIASES: &[&str] = &["armhf", "armv7", "armv7l", "armv7hl", "arm7"];
pub const I386_ALIASES: &[&str] = &["i386", "i486", "i586", "i686", "ia32", "x86", "386"];

const ALIAS_GROUPS: &[&[&str]] = &[AMD64_ALIASES, ARM64_ALIASES, ARMHF_ALIASES, I386_ALIASES];

pub fn default_arch_filter() -> Vec<String> {
    vec!["amd64".to_string()]
}

pub fn alias_group(arch: &str) -> Option<&'static [&'static str]> {
    ALIAS_GROUPS
        .iter()
        .find(|group| group.iter().any(|alias| alias.eq_ignore_ascii_case(arch)))
        .copied()
}

pub fn filename_matches_arch(filename: &str, arch: &str) -> bool {
    let aliases = alias_group(arch).unwrap_or_else(|| std::slice::from_ref(&arch));
    aliases
        .iter()
        .any(|alias| contains_arch_token(filename, alias))
}

pub fn filename_arch_priority(arch_filter: &[String], filename: &str) -> Option<usize> {
    if arch_filter.is_empty() {
        return None;
    }
    arch_filter
        .iter()
        .position(|arch| filename_matches_arch(filename, arch))
}

pub fn filter_by_filename_arch_priority<T>(
    candidates: Vec<T>,
    arch_filter: &[String],
    name_fn: impl Fn(&T) -> &str,
) -> Vec<T> {
    if arch_filter.is_empty() {
        return candidates;
    }

    let best = candidates
        .iter()
        .filter_map(|candidate| filename_arch_priority(arch_filter, name_fn(candidate)))
        .min();

    match best {
        Some(priority) => candidates
            .into_iter()
            .filter(|candidate| {
                filename_arch_priority(arch_filter, name_fn(candidate)) == Some(priority)
            })
            .collect(),
        None => candidates,
    }
}

pub fn filename_matches_filter_or_unknown(filename: &str, arch_filter: &[String]) -> bool {
    if arch_filter.is_empty() || filename_arch_priority(arch_filter, filename).is_some() {
        return true;
    }
    !ALIAS_GROUPS
        .iter()
        .flat_map(|group| group.iter())
        .any(|alias| contains_arch_token(filename, alias))
}

pub fn package_arch_matches_filter(architecture: &str, arch_filter: &[String]) -> bool {
    if arch_filter.is_empty() || architecture == "all" || architecture == "noarch" {
        return true;
    }
    arch_filter
        .iter()
        .any(|configured| architecture_matches(configured, architecture))
}

pub fn deb_architectures(arch_filter: &[String]) -> Vec<String> {
    arch_filter
        .iter()
        .map(|arch| canonical_deb_arch(arch).to_string())
        .collect()
}

pub fn rpm_architectures(arch_filter: &[String]) -> Vec<String> {
    let mut arches: Vec<String> = arch_filter
        .iter()
        .map(|arch| canonical_rpm_arch(arch).to_string())
        .collect();
    if !arches.iter().any(|arch| arch == "noarch") {
        arches.push("noarch".to_string());
    }
    arches
}

fn architecture_matches(configured: &str, actual: &str) -> bool {
    if configured.eq_ignore_ascii_case(actual) {
        return true;
    }
    let configured_group = alias_group(configured);
    let actual_group = alias_group(actual);
    configured_group.is_some() && configured_group == actual_group
}

fn canonical_deb_arch(arch: &str) -> &str {
    if architecture_matches("amd64", arch) {
        "amd64"
    } else if architecture_matches("arm64", arch) {
        "arm64"
    } else if architecture_matches("armhf", arch) {
        "armhf"
    } else if architecture_matches("i386", arch) {
        "i386"
    } else {
        arch
    }
}

fn canonical_rpm_arch(arch: &str) -> &str {
    if architecture_matches("amd64", arch) {
        "x86_64"
    } else if architecture_matches("arm64", arch) {
        "aarch64"
    } else if architecture_matches("armhf", arch) {
        "armv7hl"
    } else if architecture_matches("i386", arch) {
        "i686"
    } else {
        arch
    }
}

fn contains_arch_token(value: &str, alias: &str) -> bool {
    let value = value.to_ascii_lowercase();
    let alias = alias.to_ascii_lowercase();
    let mut start = 0;

    while let Some(pos) = value[start..].find(&alias) {
        let index = start + pos;
        let before = value[..index].chars().next_back();
        let after_index = index + alias.len();
        let after = value[after_index..].chars().next();

        if is_boundary(before)
            && is_boundary(after)
            && !ambiguous_x86_32_match(&alias, &value[after_index..])
        {
            return true;
        }
        start = after_index;
    }
    false
}

fn is_boundary(ch: Option<char>) -> bool {
    ch.is_none_or(|ch| !ch.is_ascii_alphanumeric())
}

fn ambiguous_x86_32_match(alias: &str, suffix: &str) -> bool {
    matches!(alias, "x86" | "386")
        && (suffix.starts_with("_64") || suffix.starts_with("-64") || suffix.starts_with("64"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn alias_groups_cover_common_names() {
        assert_eq!(alias_group("x64"), Some(AMD64_ALIASES));
        assert_eq!(alias_group("aarch64"), Some(ARM64_ALIASES));
        assert_eq!(alias_group("arm7"), Some(ARMHF_ALIASES));
        assert_eq!(alias_group("i686"), Some(I386_ALIASES));
        assert_eq!(alias_group("x32"), None);
    }

    #[test]
    fn filename_priority_matches_aliases() {
        assert_eq!(
            filename_arch_priority(&["amd64".into()], "tool-linux-x64.deb"),
            Some(0)
        );
        assert_eq!(
            filename_arch_priority(&["arm64".into()], "tool-arm-64.deb"),
            Some(0)
        );
        assert_eq!(
            filename_arch_priority(&["armhf".into()], "tool-armv7l.deb"),
            Some(0)
        );
        assert_eq!(
            filename_arch_priority(&["i386".into()], "tool-linux-x86.deb"),
            Some(0)
        );
        assert_eq!(
            filename_arch_priority(&["i386".into()], "tool-i686.rpm"),
            Some(0)
        );
    }

    #[test]
    fn filename_priority_avoids_false_positives() {
        assert_eq!(
            filename_arch_priority(&["i386".into()], "tool-x86_64.deb"),
            None
        );
        assert_eq!(
            filename_arch_priority(&["i386".into()], "tool-x86-64.rpm"),
            None
        );
        assert_eq!(
            filename_arch_priority(&["armhf".into()], "tool-arm64.deb"),
            None
        );
        assert_eq!(
            filename_arch_priority(&["armhf".into()], "tool-aarch64.rpm"),
            None
        );
    }

    #[test]
    fn filter_by_priority_keeps_best_arch_or_unknown_fallback() {
        let items = vec!["tool-arm64.deb", "tool-x64.deb"];
        assert_eq!(
            filter_by_filename_arch_priority(items, &["amd64".into(), "arm64".into()], |s| s),
            vec!["tool-x64.deb"]
        );

        let unknown = vec!["tool-linux.deb"];
        assert_eq!(
            filter_by_filename_arch_priority(unknown, &["amd64".into()], |s| s),
            vec!["tool-linux.deb"]
        );
    }

    #[test]
    fn package_arch_matches_metadata_aliases() {
        assert!(package_arch_matches_filter("x86_64", &["amd64".into()]));
        assert!(package_arch_matches_filter("aarch64", &["arm64".into()]));
        assert!(package_arch_matches_filter("i686", &["i386".into()]));
        assert!(package_arch_matches_filter("noarch", &["amd64".into()]));
        assert!(!package_arch_matches_filter("aarch64", &["amd64".into()]));
    }

    #[test]
    fn repository_architectures_are_canonicalized() {
        assert_eq!(
            deb_architectures(&["x64".into(), "i686".into()]),
            vec!["amd64", "i386"]
        );
        assert_eq!(
            rpm_architectures(&["amd64".into(), "arm64".into()]),
            vec!["x86_64", "aarch64", "noarch"]
        );
    }
}
