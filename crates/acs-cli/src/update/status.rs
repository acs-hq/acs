//! What `acs doctor` says about updating. It reads the settings and the state file and never
//! touches the network.

use acs_store::AcsHome;
use semver::Version;

use super::Overrides;
use super::install::{self, Method};
use super::store::{LastError, LastUpdate, Settings, State};
use super::version::parse_version;

/// Whether and how this install updates itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Updating {
    /// Not a release build: nothing here updates it.
    Never,
    /// Replaces itself; `auto` is on.
    On,
    /// `auto = false`.
    Off,
    /// Replaces itself, but only to this exact version.
    Pinned(String),
    /// A notice only: this install cannot replace itself.
    NoticeOnly,
}

/// Everything doctor shows.
#[derive(Debug, Clone)]
pub struct Summary {
    /// The version running.
    pub running: String,
    /// How it was installed.
    pub method: Method,
    /// Whether and how it updates.
    pub updating: Updating,
    /// A settings file that could not be read.
    pub settings_error: Option<String>,
    /// The newest version the last check saw.
    pub newest_known: Option<String>,
    /// When the last check finished (or started, if it never finished).
    pub checked_at: Option<i64>,
    /// The last failure.
    pub last_error: Option<LastError>,
    /// The last update.
    pub last_update: Option<LastUpdate>,
}

/// How serious what doctor found is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    /// Nothing to do.
    Pass,
    /// Worth knowing, not a failure.
    Warn,
}

/// The doctor check: its level, message and hint.
#[derive(Debug, Clone)]
pub struct Report {
    /// How serious.
    pub level: Level,
    /// What was found.
    pub message: String,
    /// What to do about it.
    pub hint: Option<String>,
}

/// Reads the facts.
#[must_use]
pub fn summarize(home: Option<&AcsHome>) -> Summary {
    let overrides = Overrides::current();
    let enabled = overrides.build_enabled();
    let method = install::current(enabled);
    let (settings, settings_error) = match home.map(Settings::load) {
        Some(Ok(settings)) => (settings, None),
        Some(Err(why)) => (Settings::default(), Some(why)),
        None => (Settings::default(), None),
    };
    let state = home.map(State::load).unwrap_or_default();
    let updating = match (&method, enabled) {
        (_, false) => Updating::Never,
        (Method::Installer, true) if settings_error.is_some() || !settings.auto => Updating::Off,
        (Method::Installer, true) => match &settings.pin {
            Some(pin) => Updating::Pinned(pin.trim_start_matches('v').to_owned()),
            None => Updating::On,
        },
        _ => Updating::NoticeOnly,
    };
    Summary {
        running: overrides.running_version(),
        method,
        updating,
        settings_error,
        newest_known: state.newest_known,
        checked_at: state.check_finished_at.or(state.check_started_at),
        last_error: state.last_error,
        last_update: state.last_update,
    }
}

/// "3 hours ago" and the like.
#[must_use]
pub fn ago(now: i64, at: i64) -> String {
    let secs = now.saturating_sub(at);
    let (n, unit) = match secs {
        ..60 => return "just now".to_owned(),
        60..3_600 => (secs / 60, "minute"),
        3_600..86_400 => (secs / 3_600, "hour"),
        _ => (secs / 86_400, "day"),
    };
    format!("{n} {unit}{} ago", if n == 1 { "" } else { "s" })
}

impl Summary {
    /// Whether the newest known version is newer than the running one and will not be installed
    /// on its own.
    #[must_use]
    pub fn waiting_notice(&self) -> Option<&str> {
        let newest = self.newest_known.as_deref()?;
        let newest_version = parse_version(newest).ok()?;
        let running: Version = parse_version(&self.running).ok()?;
        if newest_version <= running {
            return None;
        }
        let on_its_own = match &self.updating {
            Updating::On => newest_version.major == running.major,
            Updating::Never | Updating::Off | Updating::Pinned(_) | Updating::NoticeOnly => false,
        };
        // A source build cannot act on a notice; do not nag a developer.
        (!on_its_own && self.updating != Updating::Never).then_some(newest)
    }

    /// The doctor check.
    #[must_use]
    pub fn report(&self, now: i64) -> Report {
        let mut text = format!(
            "acs {}, installed with {}. ",
            self.running,
            self.method.label()
        );
        text.push_str(&match &self.updating {
            Updating::Never => {
                "This is not a release build, so it never updates itself.".to_owned()
            }
            Updating::On => "Updating is on.".to_owned(),
            Updating::Off => "Updating is off.".to_owned(),
            Updating::Pinned(v) => format!("Updating is pinned to {v}."),
            Updating::NoticeOnly => "It does not update itself; a notice only.".to_owned(),
        });
        if let Some(why) = &self.settings_error {
            text.push_str(&format!(
                " The settings file is unusable ({why}), so nothing is contacted."
            ));
        }
        match (&self.newest_known, self.checked_at) {
            (Some(newest), Some(at)) => {
                text.push_str(&format!(
                    " Newest known: {newest}, checked {}.",
                    ago(now, at)
                ));
            }
            (None, Some(at)) => {
                text.push_str(&format!(
                    " Last checked {}, no release known.",
                    ago(now, at)
                ));
            }
            _ if self.updating != Updating::Never && self.updating != Updating::Off => {
                text.push_str(" Not checked yet.");
            }
            _ => {}
        }
        if let Some(update) = &self.last_update {
            text.push_str(&format!(
                " Last update: {} to {}, {}.",
                update.from,
                update.to,
                ago(now, update.at)
            ));
        }
        let mut level = Level::Pass;
        let mut hint = None;
        if let Some(error) = &self.last_error {
            level = Level::Warn;
            text.push_str(&format!(
                " Last check failed {}: {}",
                ago(now, error.at),
                error.message
            ));
        }
        if let Some(newest) = self.waiting_notice() {
            level = Level::Warn;
            hint = Some(
                match (&self.updating, self.method.manual_update_command()) {
                    (Updating::NoticeOnly, Some(command)) => {
                        format!(
                            "{newest} is available and is not installed automatically: {command}"
                        )
                    }
                    (Updating::Off, _) => {
                        format!("{newest} is available; updating is off. `acs update` installs it")
                    }
                    (Updating::Pinned(_), _) => {
                        format!("{newest} is available; `acs update unpin` lets it update")
                    }
                    _ => format!("{newest} is available; `acs update` installs it"),
                },
            );
        }
        Report {
            level,
            message: text,
            hint,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn summary(updating: Updating, method: Method) -> Summary {
        Summary {
            running: "1.0.0".to_owned(),
            method,
            updating,
            settings_error: None,
            newest_known: Some("1.0.0".to_owned()),
            checked_at: Some(1_000),
            last_error: None,
            last_update: None,
        }
    }

    #[test]
    fn times_are_said_in_the_largest_unit() {
        assert_eq!(ago(100, 90), "just now");
        assert_eq!(ago(200, 100), "1 minute ago");
        assert_eq!(ago(100_000, 100), "1 day ago");
        assert_eq!(ago(1_000_000, 100), "11 days ago");
        assert_eq!(ago(20_000, 100), "5 hours ago");
    }

    #[test]
    fn an_up_to_date_install_passes() {
        let report = summary(Updating::On, Method::Installer).report(2_000);
        assert_eq!(report.level, Level::Pass);
        assert!(
            report.message.contains("Updating is on"),
            "{}",
            report.message
        );
        assert!(
            report.message.contains("Newest known: 1.0.0"),
            "{}",
            report.message
        );
        assert!(report.hint.is_none());
    }

    #[test]
    fn a_failure_warns_and_shows_the_message() {
        let mut s = summary(Updating::On, Method::Installer);
        s.last_error = Some(LastError {
            at: 1_500,
            message: "the signature does not match the download".to_owned(),
        });
        let report = s.report(2_000);
        assert_eq!(report.level, Level::Warn);
        assert!(report.message.contains("signature does not match"));
    }

    #[test]
    fn a_notice_only_install_names_its_command() {
        let mut s = summary(Updating::NoticeOnly, Method::Scoop);
        s.newest_known = Some("1.0.1".to_owned());
        let report = s.report(2_000);
        assert_eq!(report.level, Level::Warn);
        assert!(report.hint.unwrap().contains("scoop update acs"));
        let mut other = summary(Updating::NoticeOnly, Method::Other);
        other.newest_known = Some("1.0.1".to_owned());
        assert!(other.report(2_000).hint.unwrap().contains("installer"));
    }

    #[test]
    fn what_will_update_on_its_own_is_not_a_warning() {
        let mut s = summary(Updating::On, Method::Installer);
        s.newest_known = Some("1.0.1".to_owned());
        assert_eq!(s.report(2_000).level, Level::Pass);
        s.newest_known = Some("2.0.0".to_owned());
        assert_eq!(s.report(2_000).level, Level::Warn);
        let mut off = summary(Updating::Off, Method::Installer);
        off.newest_known = Some("1.0.1".to_owned());
        assert_eq!(off.report(2_000).level, Level::Warn);
        let mut pinned = summary(Updating::Pinned("1.0.0".to_owned()), Method::Installer);
        pinned.newest_known = Some("1.0.1".to_owned());
        assert!(pinned.report(2_000).hint.unwrap().contains("unpin"));
    }

    #[test]
    fn a_source_build_never_warns_about_releases() {
        let mut s = summary(Updating::Never, Method::Source);
        s.newest_known = Some("2.0.0".to_owned());
        let report = s.report(2_000);
        assert_eq!(report.level, Level::Pass);
        assert!(report.message.contains("not a release build"));
    }

    #[test]
    fn summarize_without_a_home_still_reports() {
        let s = summarize(None);
        assert!(!s.running.is_empty());
        let _ = s.report(0);
    }
}
