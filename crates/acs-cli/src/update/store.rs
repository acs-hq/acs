//! The updater's files under `ACS_HOME\update\`: `settings.toml` (what a person chose),
//! `state.json` (what the checks found and did) and `check.lock` (one check at a time).
//!
//! Both files are read by newer and older `acs` versions side by side, so a new field must be
//! optional for readers (decision 0054, "State and policy formats").

use std::fs::{self, File, OpenOptions, TryLockError};
use std::io;
use std::path::{Path, PathBuf};

use acs_store::AcsHome;
use serde::{Deserialize, Serialize};

use super::version::parse_version;

/// Seconds between the starts of two background checks.
pub const CHECK_INTERVAL_SECS: i64 = 24 * 60 * 60;

/// The folder of the updater's files.
#[must_use]
pub fn dir(home: &AcsHome) -> PathBuf {
    home.root().join("update")
}

/// What a person chose.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Settings {
    /// Whether the background check and update run. `false`: no contact with GitHub at all.
    #[serde(default = "yes")]
    pub auto: bool,
    /// A version to stay on: exactly that, and nothing after it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pin: Option<String>,
}

fn yes() -> bool {
    true
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            auto: true,
            pin: None,
        }
    }
}

impl Settings {
    /// The settings file of `home`.
    #[must_use]
    pub fn path(home: &AcsHome) -> PathBuf {
        dir(home).join("settings.toml")
    }

    /// Reads the settings: defaults when there is no file.
    ///
    /// # Errors
    ///
    /// A message when the file exists but cannot be read or understood. Callers that would reach
    /// the network treat that as "off".
    pub fn load(home: &AcsHome) -> Result<Self, String> {
        let path = Self::path(home);
        let text = match fs::read_to_string(&path) {
            Ok(text) => text,
            Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(Self::default()),
            Err(err) => return Err(format!("cannot read {}: {err}", path.display())),
        };
        let settings: Self = toml::from_str(&text)
            .map_err(|err| format!("{} is not valid: {err}", path.display()))?;
        if let Some(pin) = &settings.pin {
            parse_version(pin).map_err(|err| format!("{}: pin: {err}", path.display()))?;
        }
        Ok(settings)
    }

    /// Writes the settings file.
    ///
    /// # Errors
    ///
    /// The underlying I/O error.
    pub fn save(&self, home: &AcsHome) -> io::Result<()> {
        let dir = dir(home);
        fs::create_dir_all(&dir)?;
        let mut text = String::from(
            "# Set by `acs update on|off|pin|unpin`. `auto = false` means acs never contacts GitHub.\n",
        );
        text.push_str(&toml::to_string(self).map_err(io::Error::other)?);
        fs::write(Self::path(home), text)
    }
}

/// The last failure of a check.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LastError {
    /// When (Unix seconds).
    pub at: i64,
    /// What went wrong.
    pub message: String,
}

/// The last update that replaced the running file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LastUpdate {
    /// The version replaced.
    pub from: String,
    /// The version installed.
    pub to: String,
    /// When (Unix seconds).
    pub at: i64,
}

/// What the checks found and did.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct State {
    /// When the last check started (Unix seconds).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub check_started_at: Option<i64>,
    /// When the last check finished.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub check_finished_at: Option<i64>,
    /// The newest version the last listing showed (under the pre-release rule, any major).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub newest_known: Option<String>,
    /// The last failure, cleared by the next success.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_error: Option<LastError>,
    /// The last update.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_update: Option<LastUpdate>,
}

impl State {
    /// The state file of `home`.
    #[must_use]
    pub fn path(home: &AcsHome) -> PathBuf {
        dir(home).join("state.json")
    }

    /// Reads the state; a missing or damaged file is an empty state.
    #[must_use]
    pub fn load(home: &AcsHome) -> Self {
        fs::read(Self::path(home))
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default()
    }

    /// Writes the state (to a temporary file first, so a reader never sees half of it).
    ///
    /// # Errors
    ///
    /// The underlying I/O error.
    pub fn save(&self, home: &AcsHome) -> io::Result<()> {
        let dir = dir(home);
        fs::create_dir_all(&dir)?;
        let temp = dir.join("state.json.tmp");
        fs::write(
            &temp,
            serde_json::to_vec_pretty(self).map_err(io::Error::other)?,
        )?;
        fs::rename(&temp, Self::path(home))
    }

    /// Whether a background check is due at `now`: none started in the last 24 hours (a clock
    /// that went backwards counts as due).
    #[must_use]
    pub fn due(&self, now: i64) -> bool {
        self.check_started_at.is_none_or(|at| {
            let waited = now.saturating_sub(at);
            !(0..CHECK_INTERVAL_SECS).contains(&waited)
        })
    }
}

/// Held while a check runs; dropping it, or the process ending, lets the next one in.
#[derive(Debug)]
pub struct CheckLock(#[allow(dead_code)] File);

/// Takes the check lock without waiting: `None` says that a check is already running.
///
/// # Errors
///
/// The underlying error when the lock file cannot be opened or locked.
pub fn lock(home: &AcsHome) -> io::Result<Option<CheckLock>> {
    lock_in(&dir(home))
}

fn lock_in(dir: &Path) -> io::Result<Option<CheckLock>> {
    fs::create_dir_all(dir)?;
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(dir.join("check.lock"))?;
    match file.try_lock() {
        Ok(()) => Ok(Some(CheckLock(file))),
        Err(TryLockError::WouldBlock) => Ok(None),
        Err(TryLockError::Error(err)) => Err(err),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn home() -> (tempfile::TempDir, AcsHome) {
        let tmp = tempfile::tempdir().unwrap();
        let home = AcsHome::at(tmp.path().join("home")).unwrap();
        (tmp, home)
    }

    #[test]
    fn settings_default_to_automatic_and_round_trip() {
        let (_tmp, home) = home();
        assert_eq!(Settings::load(&home).unwrap(), Settings::default());
        let chosen = Settings {
            auto: false,
            pin: Some("1.0.1".to_owned()),
        };
        chosen.save(&home).unwrap();
        assert_eq!(Settings::load(&home).unwrap(), chosen);
        let text = fs::read_to_string(Settings::path(&home)).unwrap();
        assert!(text.contains("auto = false") && text.contains("pin = \"1.0.1\""));
    }

    #[test]
    fn a_partial_or_broken_settings_file() {
        let (_tmp, home) = home();
        fs::create_dir_all(dir(&home)).unwrap();
        fs::write(Settings::path(&home), "pin = \"v1.2.0\"\n").unwrap();
        let settings = Settings::load(&home).unwrap();
        assert!(settings.auto);
        assert_eq!(settings.pin.as_deref(), Some("v1.2.0"));
        fs::write(Settings::path(&home), "auto = maybe").unwrap();
        assert!(Settings::load(&home).is_err());
        fs::write(Settings::path(&home), "pin = \"soon\"").unwrap();
        assert!(Settings::load(&home).is_err());
    }

    #[test]
    fn state_round_trips_and_a_damaged_file_is_empty() {
        let (_tmp, home) = home();
        assert_eq!(State::load(&home), State::default());
        let state = State {
            check_started_at: Some(10),
            check_finished_at: Some(11),
            newest_known: Some("1.0.1".to_owned()),
            last_error: Some(LastError {
                at: 12,
                message: "offline".to_owned(),
            }),
            last_update: Some(LastUpdate {
                from: "1.0.0".to_owned(),
                to: "1.0.1".to_owned(),
                at: 13,
            }),
        };
        state.save(&home).unwrap();
        assert_eq!(State::load(&home), state);
        fs::write(State::path(&home), "{ nope").unwrap();
        assert_eq!(State::load(&home), State::default());
        // A newer acs may add fields; an older one reads past them.
        fs::write(
            State::path(&home),
            r#"{"check_started_at": 5, "future": [1]}"#,
        )
        .unwrap();
        assert_eq!(State::load(&home).check_started_at, Some(5));
    }

    #[test]
    fn a_check_is_due_after_24_hours_or_never_run() {
        let mut state = State::default();
        assert!(state.due(1_000));
        state.check_started_at = Some(1_000);
        assert!(!state.due(1_000));
        assert!(!state.due(1_000 + CHECK_INTERVAL_SECS - 1));
        assert!(state.due(1_000 + CHECK_INTERVAL_SECS));
        assert!(state.due(500), "a clock that went back");
    }

    #[test]
    fn only_one_check_holds_the_lock() {
        let (_tmp, home) = home();
        let first = lock(&home).unwrap();
        assert!(first.is_some());
        assert!(lock(&home).unwrap().is_none());
        drop(first);
        assert!(lock(&home).unwrap().is_some());
    }
}
