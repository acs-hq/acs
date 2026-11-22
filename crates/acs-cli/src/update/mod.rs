//! `acs` updates itself (decision 0054): a daily check of the GitHub releases, and on a Windows
//! installer install, a signed download that replaces the running file.
//!
//! - [`version`]: which release a check picks (semver order, the major and pre-release rules,
//!   pins) and the parsing of GitHub's releases listing.
//! - [`signature`]: the signed message, the compiled-in release key and the verification.
//! - [`install`]: how this `acs` was installed (installer receipt, Scoop, source build, other).
//! - [`store`]: the settings (`ACS_HOME\update\settings.toml`) and state
//!   (`ACS_HOME\update\state.json`) files and the lock.
//! - [`fetch`]: `curl.exe` and `tar.exe`, started by absolute path.
//! - [`swap`]: copying the new file next to the running one, trying it, and the two renames.
//! - [`check`]: one check from start to end, and the session-start spawn.
//! - [`status`]: what `acs doctor` and `acs update` say about it all.
//!
//! The hook path never touches the network: [`check::spawn_if_due`] reads one small file and
//! starts a detached `acs update --background`. A build without the cargo feature `self-update`
//! (everything but a `dist` build) returns before reading anything.
//!
//! # Test overrides
//!
//! Compiled only with `cfg(debug_assertions)` and ignored in release builds, so no environment
//! variable of a person's machine can point a release at another server or key:
//!
//! | Variable | Effect |
//! | --- | --- |
//! | `ACS_UPDATE_TEST_BASE_URL` | Replaces both GitHub addresses: the listing is `<base>/releases`, a file is `<base>/download/<tag>/<name>`. The only way to use `http://127.0.0.1:<port>`. |
//! | `ACS_UPDATE_TEST_PUBLIC_KEY` | The release public key, 64 hex digits. |
//! | `ACS_UPDATE_TEST_RELEASE_BUILD` | `1`: act as a build with the `self-update` feature. |
//! | `ACS_UPDATE_TEST_RUNNING_VERSION` | The version to pretend to be running. |

pub mod check;
pub mod fetch;
pub mod install;
pub mod signature;
pub mod status;
pub mod store;
pub mod swap;
pub mod version;

use std::ffi::OsString;

/// Whether this build may update itself: the cargo feature `self-update`, which only `dist` turns
/// on.
pub const BUILD_ENABLED: bool = cfg!(feature = "self-update");

/// The repository whose releases `acs` follows.
pub const REPOSITORY: &str = "acs-hq/acs";

/// The version this binary was built as.
pub const BUILT_VERSION: &str = env!("CARGO_PKG_VERSION");

/// What a debug build lets a test change. In a release build every field is the real value and
/// nothing is read from the environment.
#[derive(Debug, Clone, Default)]
pub struct Overrides {
    /// Replaces the GitHub addresses.
    pub base_url: Option<String>,
    /// The release public key (hex).
    pub public_key: Option<String>,
    /// Acts as a release build.
    pub release_build: bool,
    /// The version to pretend to run.
    pub running_version: Option<String>,
}

impl Overrides {
    /// The overrides of this process (always empty in a release build).
    #[must_use]
    pub fn current() -> Self {
        Self::from_vars(|name| std::env::var_os(name))
    }

    #[cfg(debug_assertions)]
    fn from_vars(get: impl Fn(&str) -> Option<OsString>) -> Self {
        let text = |name: &str| {
            get(name)
                .and_then(|v| v.into_string().ok())
                .filter(|v| !v.is_empty())
        };
        Self {
            base_url: text("ACS_UPDATE_TEST_BASE_URL"),
            public_key: text("ACS_UPDATE_TEST_PUBLIC_KEY"),
            release_build: text("ACS_UPDATE_TEST_RELEASE_BUILD").as_deref() == Some("1"),
            running_version: text("ACS_UPDATE_TEST_RUNNING_VERSION"),
        }
    }

    #[cfg(not(debug_assertions))]
    fn from_vars(_get: impl Fn(&str) -> Option<OsString>) -> Self {
        Self::default()
    }

    /// Whether this process acts as a release build.
    #[must_use]
    pub fn build_enabled(&self) -> bool {
        BUILD_ENABLED || self.release_build
    }

    /// The version this process says it runs.
    #[must_use]
    pub fn running_version(&self) -> String {
        self.running_version
            .clone()
            .unwrap_or_else(|| BUILT_VERSION.to_owned())
    }
}

#[cfg(all(test, debug_assertions))]
mod tests {
    use super::*;

    #[test]
    fn overrides_read_the_named_variables() {
        let vars = |name: &str| -> Option<OsString> {
            match name {
                "ACS_UPDATE_TEST_BASE_URL" => Some("http://127.0.0.1:1".into()),
                "ACS_UPDATE_TEST_RELEASE_BUILD" => Some("1".into()),
                "ACS_UPDATE_TEST_RUNNING_VERSION" => Some("0.9.0".into()),
                _ => None,
            }
        };
        let o = Overrides::from_vars(vars);
        assert_eq!(o.base_url.as_deref(), Some("http://127.0.0.1:1"));
        assert!(o.build_enabled());
        assert_eq!(o.running_version(), "0.9.0");
        assert!(o.public_key.is_none());
    }

    #[test]
    fn no_overrides_means_the_built_values() {
        let o = Overrides::from_vars(|_| None);
        assert_eq!(o.running_version(), BUILT_VERSION);
        assert_eq!(o.build_enabled(), BUILD_ENABLED);
    }
}
