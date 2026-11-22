//! `acs update`: check for a new release and install it now, and the switches around it
//! (decision 0054).
//!
//! - `acs update`: checks now, in the foreground, and reports what it did.
//! - `acs update on|off`: whether the daily background check runs (`off`: no contact with GitHub).
//! - `acs update pin <version>` / `unpin`: stay on exactly one version.
//! - `acs update --background`: what the session-start hook starts. Hidden; prints nothing.
//!
//! A person's command: self-protection denies it to agents (it is in the stateful list), and the
//! settings live under `ACS_HOME`, which agents cannot write.

use std::process::ExitCode;

use acs_store::AcsHome;
use clap::{Args, Subcommand};
use serde_json::json;

use super::support::{CliError, OutputArgs, finish, print_json, wrap};
use crate::update::check::{self, Mode, NoticeKind, Outcome};
use crate::update::store::Settings;
use crate::update::version::parse_version;

/// Arguments of `acs update`.
#[derive(Debug, Args)]
#[command(
    after_help = "Examples:\n  acs update\n  acs update off\n  acs update pin 1.0.1\n\n\
                  Only installs made by the PowerShell installer on Windows replace themselves; \
                  other installs are told which command updates them. `acs doctor` shows the \
                  state."
)]
pub struct UpdateArgs {
    /// What to change; without one, check and update now.
    #[command(subcommand)]
    pub command: Option<UpdateCommand>,
    /// The background check the session-start hook starts: silent, honours `on|off` and the
    /// daily interval.
    #[arg(long, hide = true)]
    pub background: bool,
    /// Output format.
    #[command(flatten)]
    pub output: OutputArgs,
}

/// The switches.
#[derive(Debug, Subcommand)]
pub enum UpdateCommand {
    /// Check for a new release every day (the default).
    On(OutputArgs),
    /// Never contact GitHub in the background.
    Off(OutputArgs),
    /// Stay on exactly this version, and nothing after it.
    Pin(PinArgs),
    /// Follow new releases again.
    Unpin(OutputArgs),
}

/// Arguments of `acs update pin`.
#[derive(Debug, Args)]
pub struct PinArgs {
    /// The version, for example `1.0.1`.
    #[arg(value_name = "VERSION")]
    pub version: String,
    /// Output format.
    #[command(flatten)]
    pub output: OutputArgs,
}

/// Runs `acs update`.
#[must_use]
pub fn run(args: &UpdateArgs) -> ExitCode {
    if args.background {
        // Silent by design: it runs detached from a session, and the state file records what
        // happened.
        if let Ok(home) = AcsHome::from_env() {
            let _ = check::run(&home, Mode::Background, check::now_secs());
        }
        return ExitCode::SUCCESS;
    }
    let json = match &args.command {
        None => args.output.json,
        Some(UpdateCommand::On(o) | UpdateCommand::Off(o) | UpdateCommand::Unpin(o)) => o.json,
        Some(UpdateCommand::Pin(p)) => p.output.json,
    } || args.output.json;
    finish(json, execute(args, json))
}

fn home() -> Result<AcsHome, CliError> {
    AcsHome::from_env().map_err(|err| CliError::new("acs_home", err.to_string()))
}

fn save(home: &AcsHome, settings: &Settings) -> Result<(), CliError> {
    settings
        .save(home)
        .map_err(|err| CliError::new("io", format!("cannot write the update settings: {err}")))
}

fn settings(home: &AcsHome) -> Result<Settings, CliError> {
    Settings::load(home).map_err(|why| CliError::new("settings", why))
}

fn execute(args: &UpdateArgs, json: bool) -> Result<ExitCode, CliError> {
    let home = home()?;
    let (message, extra) = match &args.command {
        None => return check_now(&home, json),
        Some(UpdateCommand::On(_)) => {
            let mut s = settings(&home)?;
            s.auto = true;
            save(&home, &s)?;
            (
                "Updating is on: acs checks for a new release once a day.".to_owned(),
                json!({"auto": true}),
            )
        }
        Some(UpdateCommand::Off(_)) => {
            let mut s = settings(&home)?;
            s.auto = false;
            save(&home, &s)?;
            (
                "Updating is off: acs no longer contacts GitHub. `acs update` still checks \
                 when you run it."
                    .to_owned(),
                json!({"auto": false}),
            )
        }
        Some(UpdateCommand::Pin(pin)) => {
            let version =
                parse_version(&pin.version).map_err(|why| CliError::new("invalid", why))?;
            let mut s = settings(&home)?;
            s.pin = Some(version.to_string());
            save(&home, &s)?;
            (
                format!(
                    "Pinned to {version}. acs stays on exactly that version. `acs update` moves \
                     to it now."
                ),
                json!({"pin": version.to_string()}),
            )
        }
        Some(UpdateCommand::Unpin(_)) => {
            let mut s = settings(&home)?;
            s.pin = None;
            save(&home, &s)?;
            (
                "Unpinned: acs follows new releases again.".to_owned(),
                json!({"pin": null}),
            )
        }
    };
    if json {
        print_json(&json!({"ok": true, "message": message, "settings": extra}));
    } else {
        print!("{}", wrap(&message, "", ""));
    }
    Ok(ExitCode::SUCCESS)
}

fn check_now(home: &AcsHome, json: bool) -> Result<ExitCode, CliError> {
    let outcome = check::run(home, Mode::Manual, check::now_secs())
        .map_err(|why| CliError::new("update_failed", why))?;
    let (result, text) = describe(&outcome);
    if json {
        print_json(
            &json!({"ok": true, "result": result, "message": text, "details": details(&outcome)}),
        );
    } else {
        print!("{}", wrap(&text, "", ""));
    }
    Ok(ExitCode::SUCCESS)
}

fn details(outcome: &Outcome) -> serde_json::Value {
    match outcome {
        Outcome::UpToDate { newest } => json!({"newest": newest}),
        Outcome::Updated { from, to } => json!({"from": from, "to": to}),
        Outcome::Notice {
            version, command, ..
        } => json!({"newest": version, "command": command}),
        _ => json!({}),
    }
}

/// A stable result word and the sentence for people.
fn describe(outcome: &Outcome) -> (&'static str, String) {
    match outcome {
        Outcome::Off(why) => ("off", format!("No check was made: {why}.")),
        Outcome::NotDue => ("not_due", "No check is due yet.".to_owned()),
        Outcome::Busy => (
            "busy",
            "Another check is already running; try again in a minute.".to_owned(),
        ),
        Outcome::UpToDate { newest } => (
            "up_to_date",
            match newest {
                Some(newest) => format!(
                    "acs {} is up to date (newest release: {newest}).",
                    crate::update::Overrides::current().running_version()
                ),
                None => "acs is up to date (no newer release found).".to_owned(),
            },
        ),
        Outcome::Updated { from, to } => (
            "updated",
            format!(
                "Updated acs from {from} to {to}. New sessions use it; the old file is removed \
                 at a later check."
            ),
        ),
        Outcome::Notice {
            version,
            kind,
            command,
        } => {
            let why = match kind {
                NoticeKind::NewerMajor => "a newer major version",
                NoticeKind::NotReplaceable => "this install does not update itself",
            };
            let how = command.as_ref().map_or_else(
                || " Reinstall it the way you installed it.".to_owned(),
                |c| format!(" Update it with: {c}"),
            );
            (
                "notice",
                format!("acs {version} is available ({why}).{how}"),
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_sentence_fits_80_columns_once_wrapped() {
        let outcomes = [
            Outcome::Off("updating is off".to_owned()),
            Outcome::NotDue,
            Outcome::Busy,
            Outcome::UpToDate {
                newest: Some("1.0.0".to_owned()),
            },
            Outcome::Updated {
                from: "1.0.0".to_owned(),
                to: "1.0.1".to_owned(),
            },
            Outcome::Notice {
                version: "1.0.1".to_owned(),
                kind: NoticeKind::NotReplaceable,
                command: Some(crate::update::install::installer_line().to_owned()),
            },
        ];
        for outcome in &outcomes {
            let (_, text) = describe(outcome);
            for line in wrap(&text, "", "").lines() {
                assert!(line.chars().count() <= 80, "{line:?}");
            }
        }
    }
}
