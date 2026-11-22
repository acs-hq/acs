//! Which release a check picks, and the parsing of GitHub's releases listing.
//!
//! The rules (decision 0054):
//!
//! - Drafts are never considered.
//! - A running pre-release (`1.0.0-rc.1`) also considers pre-releases; a stable build considers
//!   only stable releases.
//! - Automatic updates stay within the running major version. A newer major is a notice, and
//!   `acs update` (a person asking) installs it.
//! - It never moves to an older version unless a pin asks for it. A pin means exactly that
//!   version and nothing after it.

use semver::Version;
use serde::Deserialize;

/// One published release: its tag and the version the tag names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Release {
    /// The tag as published (`v1.0.0`); the downloads live under it.
    pub tag: String,
    /// The version it names.
    pub version: Version,
    /// Whether it is a pre-release (GitHub's flag, or a pre-release part in the version).
    pub prerelease: bool,
}

#[derive(Deserialize)]
struct RawRelease {
    tag_name: Option<String>,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    prerelease: bool,
}

/// Parses a version as people write it: `1.0.0` or `v1.0.0`.
///
/// # Errors
///
/// The text when it is not a semantic version.
pub fn parse_version(text: &str) -> Result<Version, String> {
    let trimmed = text.trim();
    let bare = trimmed.strip_prefix('v').unwrap_or(trimmed);
    Version::parse(bare).map_err(|err| format!("`{trimmed}` is not a version ({err})"))
}

/// Parses GitHub's `releases` listing, skipping drafts and tags that are not versions.
///
/// # Errors
///
/// A message when the text is not a JSON array.
pub fn parse_listing(json: &[u8]) -> Result<Vec<Release>, String> {
    let raw: Vec<RawRelease> = serde_json::from_slice(json)
        .map_err(|err| format!("the release list is unreadable: {err}"))?;
    Ok(raw
        .into_iter()
        .filter(|r| !r.draft)
        .filter_map(|r| {
            let tag = r.tag_name?;
            let version = parse_version(&tag).ok()?;
            Some(Release {
                prerelease: r.prerelease || !version.pre.is_empty(),
                tag,
                version,
            })
        })
        .collect())
}

/// What a check decided.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Pick {
    /// Nothing newer (or nothing the rules allow).
    UpToDate,
    /// Install this release.
    Install(Release),
    /// A newer release exists that will not be installed on its own (a newer major version).
    Notice(Release),
    /// The pin names a version that is not published.
    PinMissing(Version),
}

/// The inputs of a decision.
#[derive(Debug, Clone, Copy)]
pub struct Rules<'a> {
    /// The version running now.
    pub running: &'a Version,
    /// The pinned version, if any.
    pub pin: Option<&'a Version>,
    /// A person asked (`acs update`): a newer major version is installed too.
    pub manual: bool,
}

fn considered(release: &Release, running: &Version) -> bool {
    !release.prerelease || !running.pre.is_empty()
}

/// The newest release the pre-release rule lets through, whatever its major version: the one
/// `acs doctor` calls the newest known.
#[must_use]
pub fn newest_known<'a>(releases: &'a [Release], running: &Version) -> Option<&'a Release> {
    releases
        .iter()
        .filter(|r| considered(r, running))
        .max_by(|a, b| a.version.cmp(&b.version))
}

/// Decides what to do about `releases`.
#[must_use]
pub fn pick(releases: &[Release], rules: Rules<'_>) -> Pick {
    if let Some(pin) = rules.pin {
        // A pin is exact: that version, older or newer, pre-release or not.
        return match releases.iter().find(|r| &r.version == pin) {
            None => Pick::PinMissing(pin.clone()),
            Some(found) if &found.version == rules.running => Pick::UpToDate,
            Some(found) => Pick::Install(found.clone()),
        };
    }
    let Some(newest) = newest_known(releases, rules.running) else {
        return Pick::UpToDate;
    };
    if newest.version <= *rules.running {
        return Pick::UpToDate;
    }
    if rules.manual || newest.version.major == rules.running.major {
        return Pick::Install(newest.clone());
    }
    // The newest is a newer major version. A newer release of the running major may still exist.
    let within = releases
        .iter()
        .filter(|r| considered(r, rules.running) && r.version.major == rules.running.major)
        .max_by(|a, b| a.version.cmp(&b.version));
    match within {
        Some(r) if r.version > *rules.running => Pick::Install(r.clone()),
        _ => Pick::Notice(newest.clone()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(text: &str) -> Version {
        parse_version(text).unwrap()
    }

    fn rel(tag: &str) -> Release {
        let version = parse_version(tag).unwrap();
        Release {
            tag: tag.to_owned(),
            prerelease: !version.pre.is_empty(),
            version,
        }
    }

    fn rules<'a>(running: &'a Version, pin: Option<&'a Version>, manual: bool) -> Rules<'a> {
        Rules {
            running,
            pin,
            manual,
        }
    }

    #[test]
    fn a_newer_release_in_the_same_major_is_installed() {
        let list = [rel("v1.0.0"), rel("v1.2.0"), rel("v1.1.0")];
        let running = v("1.0.0");
        assert_eq!(
            pick(&list, rules(&running, None, false)),
            Pick::Install(rel("v1.2.0"))
        );
    }

    #[test]
    fn nothing_newer_is_up_to_date_and_never_older() {
        let list = [rel("v1.0.0"), rel("v0.9.0")];
        assert_eq!(pick(&list, rules(&v("1.0.0"), None, false)), Pick::UpToDate);
        assert_eq!(pick(&list, rules(&v("1.0.0"), None, true)), Pick::UpToDate);
        assert_eq!(pick(&[], rules(&v("1.0.0"), None, false)), Pick::UpToDate);
        assert_eq!(pick(&list, rules(&v("1.3.0"), None, false)), Pick::UpToDate);
    }

    #[test]
    fn a_stable_build_ignores_pre_releases() {
        let list = [rel("v1.0.0"), rel("v1.1.0-rc.1")];
        assert_eq!(pick(&list, rules(&v("1.0.0"), None, false)), Pick::UpToDate);
        let flagged = Release {
            prerelease: true,
            ..rel("v1.1.0")
        };
        assert_eq!(
            pick(&[rel("v1.0.0"), flagged], rules(&v("1.0.0"), None, false)),
            Pick::UpToDate
        );
        assert_eq!(newest_known(&list, &v("1.0.0")).unwrap().tag, "v1.0.0");
    }

    #[test]
    fn a_pre_release_build_follows_pre_releases_and_then_the_stable_release() {
        let list = [rel("v1.0.0-rc.1"), rel("v1.0.0-rc.2")];
        assert_eq!(
            pick(&list, rules(&v("1.0.0-rc.1"), None, false)),
            Pick::Install(rel("v1.0.0-rc.2"))
        );
        let list = [rel("v1.0.0-rc.1"), rel("v1.0.0-rc.2"), rel("v1.0.0")];
        assert_eq!(
            pick(&list, rules(&v("1.0.0-rc.2"), None, false)),
            Pick::Install(rel("v1.0.0"))
        );
    }

    #[test]
    fn a_newer_major_is_a_notice_unless_a_person_asks() {
        let list = [rel("v1.0.0"), rel("v2.0.0")];
        let running = v("1.0.0");
        assert_eq!(
            pick(&list, rules(&running, None, false)),
            Pick::Notice(rel("v2.0.0"))
        );
        assert_eq!(
            pick(&list, rules(&running, None, true)),
            Pick::Install(rel("v2.0.0"))
        );
    }

    #[test]
    fn a_newer_major_does_not_hide_a_newer_release_of_the_running_major() {
        let list = [rel("v1.0.0"), rel("v1.0.1"), rel("v2.0.0")];
        assert_eq!(
            pick(&list, rules(&v("1.0.0"), None, false)),
            Pick::Install(rel("v1.0.1"))
        );
    }

    #[test]
    fn a_pin_is_exactly_that_version() {
        let list = [rel("v1.0.0"), rel("v1.1.0"), rel("v1.2.0")];
        let pin = v("1.1.0");
        assert_eq!(
            pick(&list, rules(&v("1.0.0"), Some(&pin), false)),
            Pick::Install(rel("v1.1.0"))
        );
        // Already on it: nothing after it is considered.
        assert_eq!(pick(&list, rules(&pin, Some(&pin), false)), Pick::UpToDate);
        // A pin to an older version moves back to it.
        assert_eq!(
            pick(&list, rules(&v("1.2.0"), Some(&pin), false)),
            Pick::Install(rel("v1.1.0"))
        );
        let missing = v("1.5.0");
        assert_eq!(
            pick(&list, rules(&v("1.0.0"), Some(&missing), false)),
            Pick::PinMissing(missing.clone())
        );
    }

    #[test]
    fn a_pin_may_name_a_pre_release_or_another_major() {
        let list = [rel("v1.0.0"), rel("v2.0.0-rc.1")];
        let pin = v("2.0.0-rc.1");
        assert_eq!(
            pick(&list, rules(&v("1.0.0"), Some(&pin), false)),
            Pick::Install(rel("v2.0.0-rc.1"))
        );
    }

    #[test]
    fn the_listing_skips_drafts_and_odd_tags() {
        let json = br#"[
            {"tag_name": "v1.1.0", "draft": true, "prerelease": false, "name": "x"},
            {"tag_name": "v1.0.1", "draft": false, "prerelease": false},
            {"tag_name": "nightly", "draft": false, "prerelease": true},
            {"tag_name": "v1.0.0-rc.1", "draft": false, "prerelease": true},
            {"draft": false}
        ]"#;
        let list = parse_listing(json).unwrap();
        let tags: Vec<_> = list.iter().map(|r| r.tag.as_str()).collect();
        assert_eq!(tags, ["v1.0.1", "v1.0.0-rc.1"]);
        assert!(list[1].prerelease && !list[0].prerelease);
        assert!(parse_listing(b"{\"message\": \"rate limited\"}").is_err());
        assert!(parse_listing(b"").is_err());
    }

    #[test]
    fn versions_parse_with_or_without_a_v() {
        assert_eq!(v("v1.2.3"), v("1.2.3"));
        assert!(parse_version("one").is_err());
    }
}
