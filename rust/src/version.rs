//! The app's version and where its releases are published.
//!
//! Version text follows the existing app's rules: the first run of up to
//! three dot-separated numbers in a string is the version, and text without
//! one counts as the development default.

use std::cmp::Ordering;

/// What a development build reports, and what unreadable version text means.
pub const BASE_VERSION: &str = "0.1.0";

/// The only release asset this build installs. The C# builds publish
/// `CodexUsageTray.exe` and `CodexUsageTray-Wpf.zip` in the same releases;
/// neither is ever downloaded here.
pub const RELEASE_ASSET_NAME: &str = "usage-tray-rs.exe";

pub const GITHUB_OWNER: &str = "jeremygold02";
pub const GITHUB_REPO: &str = "codex-usage-tray-icon";

/// The version of this build: the package version, unless the build set
/// `USAGE_TRAY_VERSION` to name the release it was made from. The C# app
/// honours its equivalent, `LocalWpfVersion`, only on development builds;
/// here the variable always wins, because nothing else stamps a release.
pub fn current() -> String {
    resolve(env!("CARGO_PKG_VERSION"), option_env!("USAGE_TRAY_VERSION"))
}

fn resolve(package: &str, build_override: Option<&str>) -> String {
    build_override
        .and_then(find_version)
        .map_or_else(|| display(package), str::to_string)
}

/// The version contained in `value`, such as `1.4.2` for the tag `v1.4.2`.
pub fn display(value: &str) -> String {
    find_version(value).unwrap_or(BASE_VERSION).to_string()
}

/// Orders two version texts by major, minor and patch number.
pub fn compare(left: &str, right: &str) -> Ordering {
    parts(left).cmp(&parts(right))
}

/// The repository's web address.
pub fn repository_url() -> String {
    format!("https://github.com/{GITHUB_OWNER}/{GITHUB_REPO}")
}

/// The repository's address in the GitHub REST API.
pub fn api_url() -> String {
    format!("https://api.github.com/repos/{GITHUB_OWNER}/{GITHUB_REPO}")
}

/// The first `digits[.digits[.digits]]` run in `value`.
fn find_version(value: &str) -> Option<&str> {
    let bytes = value.as_bytes();
    let digits_end = |from: usize| {
        from + bytes[from..]
            .iter()
            .take_while(|byte| byte.is_ascii_digit())
            .count()
    };
    let start = bytes.iter().position(u8::is_ascii_digit)?;
    let mut end = digits_end(start);
    for _ in 0..2 {
        let another_part =
            bytes.get(end) == Some(&b'.') && bytes.get(end + 1).is_some_and(u8::is_ascii_digit);
        if !another_part {
            break;
        }
        end = digits_end(end + 1);
    }
    Some(&value[start..end])
}

/// Missing parts are zero. So is a part too large for a number, as in the C# app.
fn parts(value: &str) -> [i32; 3] {
    let mut parts = [0; 3];
    let version = display(value);
    for (part, text) in parts.iter_mut().zip(version.split('.')) {
        *part = text.parse().unwrap_or(0);
    }
    parts
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_are_extracted_from_tags_and_free_text() {
        assert_eq!(display("1.4.2"), "1.4.2");
        assert_eq!(display("v1.4.2"), "1.4.2");
        assert_eq!(display("release-2026.11"), "2026.11");
        assert_eq!(display("7"), "7");
        assert_eq!(display("1.2.3.4"), "1.2.3");
        assert_eq!(display("v1.2-beta.5"), "1.2");
        assert_eq!(display("1."), "1");
        assert_eq!(display("1..2"), "1");
        assert_eq!(display("build 12 of 3.4"), "12");
    }

    #[test]
    fn text_without_a_version_is_the_development_default() {
        assert_eq!(display(""), BASE_VERSION);
        assert_eq!(display("latest"), BASE_VERSION);
        assert_eq!(display("v.x"), BASE_VERSION);
    }

    #[test]
    fn versions_compare_by_number_not_by_text() {
        assert_eq!(compare("1.10.0", "1.9.9"), Ordering::Greater);
        assert_eq!(compare("1.2.3", "1.2.4"), Ordering::Less);
        assert_eq!(compare("2.0.0", "10.0.0"), Ordering::Less);
        assert_eq!(compare("v2", "1.99.99"), Ordering::Greater);
        assert_eq!(compare("0.1.1", "0.1.0"), Ordering::Greater);
    }

    #[test]
    fn missing_parts_and_decoration_do_not_affect_the_order() {
        assert_eq!(compare("1.2", "1.2.0"), Ordering::Equal);
        assert_eq!(compare("v1.2.0", "1.2"), Ordering::Equal);
        assert_eq!(compare("1.2.3.9", "1.2.3"), Ordering::Equal);
        assert_eq!(compare("007", "7.0.0"), Ordering::Equal);
        assert_eq!(compare("unknown", BASE_VERSION), Ordering::Equal);
    }

    #[test]
    fn a_part_too_large_for_a_number_counts_as_zero() {
        assert_eq!(compare("99999999999.1", "0.1.0"), Ordering::Equal);
        assert_eq!(compare("99999999999.1", "0.2"), Ordering::Less);
    }

    #[test]
    fn the_baked_version_is_used_unless_the_build_names_a_release() {
        assert_eq!(resolve("1.4.2", None), display("1.4.2"));
        assert_eq!(resolve("0.1.0", Some("v2.3.1")), "2.3.1");
        assert_eq!(resolve("1.4.2", Some("2.0")), "2.0");
        assert_eq!(resolve("1.4.2", Some("")), "1.4.2");
        assert_eq!(resolve("1.4.2", Some("local")), "1.4.2");
        if option_env!("USAGE_TRAY_VERSION").is_none() {
            assert_eq!(current(), display(env!("CARGO_PKG_VERSION")));
        }
    }

    #[test]
    fn releases_come_from_the_existing_repository() {
        assert_eq!(
            repository_url(),
            "https://github.com/jeremygold02/codex-usage-tray-icon"
        );
        assert_eq!(
            api_url(),
            "https://api.github.com/repos/jeremygold02/codex-usage-tray-icon"
        );
        assert_eq!(RELEASE_ASSET_NAME, "usage-tray-rs.exe");
    }
}
