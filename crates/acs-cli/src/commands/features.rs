//! `acs features`: show which parts of ACS run in this repository, and switch one on or off.
//!
//! The switches are the `[features]` table of `.acs/policy/config.toml` (decision 0033). The
//! file is committed, so a switch is the same for everyone working in the repository once it is
//! pushed, and it is sealed, so an agent cannot change one: `acs features <name> on|off` is
//! denied to agents like every other command that changes ACS.
//!
//! Switching edits the one line of the file and leaves the rest, comments included, as it is.

use std::fs;
use std::path::Path;
use std::process::ExitCode;

use acs_core::{Config, Feature};
use clap::{Args, ValueEnum};
use serde_json::json;

use super::init::remotes;
use super::support::{CliError, OutputArgs, finish, open_repo, print_json, wrap};

/// Arguments of `acs features`.
#[derive(Debug, Args)]
#[command(
    after_help = "Features: claims, guards, territory, lore, history, commit_checks, sync.\n\
                  Without arguments, lists them. The switches live in the [features] table of \
                  .acs/policy/config.toml; commit that file after changing one.\n\n\
                  ACS's protection of itself is not a feature: agents can never edit .acs/, the \
                  git hooks or the harness's hook settings, whatever is switched off."
)]
pub struct FeaturesArgs {
    /// The feature to switch.
    #[arg(value_name = "FEATURE", requires = "state")]
    pub feature: Option<String>,
    /// `on` or `off`.
    #[arg(value_name = "on|off", value_enum)]
    pub state: Option<Switch>,
    /// Output format.
    #[command(flatten)]
    pub output: OutputArgs,
}

/// The position of a switch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum Switch {
    /// Switch the feature on.
    On,
    /// Switch the feature off.
    Off,
}

/// Runs `acs features`.
#[must_use]
pub fn run(args: &FeaturesArgs) -> ExitCode {
    finish(args.output.json, execute(args))
}

fn execute(args: &FeaturesArgs) -> Result<ExitCode, CliError> {
    let (ctx, _cwd) = open_repo()?;
    let json = args.output.json;
    let (Some(name), Some(state)) = (&args.feature, args.state) else {
        let features = ctx.features();
        if json {
            let list: Vec<_> = Feature::ALL
                .into_iter()
                .map(|f| json!({"feature": f.key(), "on": features.is_on(f), "what": f.summary()}))
                .collect();
            print_json(&json!({"ok": true, "features": list}));
        } else {
            print!("{}", render(&features));
        }
        return Ok(ExitCode::SUCCESS);
    };
    let feature =
        Feature::from_key(&name.to_ascii_lowercase().replace('-', "_")).ok_or_else(|| {
            let known: Vec<&str> = Feature::ALL.into_iter().map(Feature::key).collect();
            CliError::new(
                "invalid",
                format!(
                    "There is no feature `{name}`. The features are: {}.",
                    known.join(", ")
                ),
            )
        })?;
    let on = state == Switch::On;
    let work_tree = &ctx.layout.work_tree;
    let file = work_tree.join(".acs").join("policy").join("config.toml");
    let changed = switch_in_file(&file, feature, on)?;

    let mut notes: Vec<String> = Vec::new();
    if feature == Feature::Sync && on {
        // What `acs init` does for a repository that starts out with sync on.
        match remotes::list(work_tree) {
            Ok(names) if names.is_empty() => notes.push(
                "This repository has no git remote yet, so there is nothing to sync with."
                    .to_owned(),
            ),
            Ok(names) => {
                for name in names {
                    if let Err(err) = remotes::ensure(work_tree, &name) {
                        notes.push(format!("remote `{name}`: {err}"));
                    }
                }
                notes.push(
                    "Sync publishes claimed file paths, session records and your git email to \
                     `refs/acs/*` on the remote. Teammates run `acs init` once after pulling \
                     this change."
                        .to_owned(),
                );
            }
            Err(err) => notes.push(err),
        }
    }
    if json {
        print_json(&json!({
            "ok": true,
            "feature": feature.key(),
            "on": on,
            "changed": changed,
            "file": ".acs/policy/config.toml",
            "notes": notes,
        }));
    } else {
        let word = if on { "on" } else { "off" };
        let text = if changed {
            format!(
                "{feature} is now {word} in this checkout. Commit `.acs/policy/config.toml` so \
                 that it holds for everyone working in the repository."
            )
        } else {
            format!("{feature} was already {word}.")
        };
        print!("{}", wrap(&text, "", ""));
        for note in &notes {
            print!("{}", wrap(note, "  ", "  "));
        }
    }
    Ok(ExitCode::SUCCESS)
}

/// The list `acs features` prints.
#[must_use]
pub fn render(features: &acs_core::Features) -> String {
    let mut out = String::from("ACS features in this repository\n");
    for feature in Feature::ALL {
        let state = if features.is_on(feature) { "on" } else { "off" };
        // `wrap` collapses runs of spaces, so the name column is part of the prefix.
        out.push_str(&wrap(
            feature.summary(),
            &format!("  {state:<5}{:<15}", feature.key()),
            &" ".repeat(22),
        ));
    }
    out.push('\n');
    out.push_str(&wrap(
        "Switch one with `acs features <name> on|off`, or edit the [features] table of \
         `.acs/policy/config.toml`; then commit that file. ACS's protection of its own files, \
         hooks and state is always on.",
        "",
        "",
    ));
    out
}

/// Sets `feature` in the config file. Returns whether the file changed.
fn switch_in_file(file: &Path, feature: Feature, on: bool) -> Result<bool, CliError> {
    let text = fs::read_to_string(file).map_err(|err| {
        CliError::new(
            "no_policy",
            format!(
                "cannot read `.acs/policy/config.toml` ({err}). Run `acs init` first if this \
                 repository has not adopted ACS."
            ),
        )
    })?;
    let by_hand = |why: String| {
        CliError::new(
            "invalid",
            format!(
                "{why} Edit `.acs/policy/config.toml` by hand: under `[features]`, set `{} = \
                 {on}`.",
                feature.key()
            ),
        )
    };
    let before: Config = toml::from_str(&text)
        .map_err(|err| by_hand(format!("The file is not valid as it is ({err}).")))?;
    if before.features.is_on(feature) == on {
        return Ok(false);
    }
    let edited = set_in_text(&text, feature.key(), on);
    // The edit works on lines; a file that writes the table another way (inline, dotted keys)
    // is caught here instead of being written wrongly.
    let ok = toml::from_str::<Config>(&edited).is_ok_and(|after| {
        let mut expected = before.features;
        match feature {
            Feature::Claims => expected.claims = on,
            Feature::Guards => expected.guards = on,
            Feature::Territory => expected.territory = on,
            Feature::Lore => expected.lore = on,
            Feature::History => expected.history = on,
            Feature::CommitChecks => expected.commit_checks = on,
            Feature::Sync => expected.sync = on,
        }
        after.features == expected
            && Config {
                features: before.features,
                ..after
            } == before
    });
    if !ok {
        return Err(by_hand(
            "The [features] table is written in a way this command cannot edit safely.".to_owned(),
        ));
    }
    fs::write(file, edited).map_err(|err| {
        CliError::new(
            "io",
            format!("cannot write `.acs/policy/config.toml`: {err}"),
        )
    })?;
    Ok(true)
}

/// The name of the table a line opens (`[features]`), if it opens one.
fn table_name(line: &str) -> Option<&str> {
    let line = line.split('#').next().unwrap_or("").trim();
    let inner = line.strip_prefix('[')?.strip_suffix(']')?;
    (!inner.starts_with('[')).then(|| inner.trim())
}

/// The key a `key = value` line sets, if it is one.
fn key_of(line: &str) -> Option<&str> {
    let line = line.trim_start();
    if line.starts_with('#') || line.starts_with('[') {
        return None;
    }
    line.split_once('=').map(|(key, _)| key.trim())
}

/// `text` with `key` set to `on` in its `[features]` table: the line is rewritten if it exists,
/// added to the table if not, and the table is added at the end of the file if there is none.
/// Everything else, comments and line endings included, stays as it is.
fn set_in_text(text: &str, key: &str, on: bool) -> String {
    let eol = if text.contains("\r\n") { "\r\n" } else { "\n" };
    let mut lines: Vec<String> = text.split_inclusive('\n').map(str::to_owned).collect();
    if let Some(last) = lines.last_mut()
        && !last.ends_with('\n')
    {
        last.push_str(eol);
    }
    let Some(header) = lines
        .iter()
        .position(|line| table_name(line) == Some("features"))
    else {
        if !lines.is_empty() {
            lines.push(eol.to_owned());
        }
        lines.push(format!("[features]{eol}"));
        lines.push(format!("{key} = {on}{eol}"));
        return lines.concat();
    };
    let end = lines[header + 1..]
        .iter()
        .position(|line| line.trim_start().starts_with('['))
        .map_or(lines.len(), |offset| header + 1 + offset);
    if let Some(at) = (header + 1..end).find(|&i| key_of(&lines[i]) == Some(key)) {
        let line = &lines[at];
        let assign = line.find('=').map_or(line.len(), |i| i + 1);
        let comment = line[assign..]
            .find('#')
            .map(|i| format!("  {}", line[assign + i..].trim_end()))
            .unwrap_or_default();
        lines[at] = format!("{} {on}{comment}{eol}", &line[..assign]);
    } else {
        let after = (header..end)
            .rev()
            .find(|&i| !lines[i].trim().is_empty())
            .unwrap_or(header);
        lines.insert(after + 1, format!("{key} = {on}{eol}"));
    }
    lines.concat()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rewrites_the_line_and_keeps_everything_else() {
        let text = "spec = \"acs/0.1\"\n\n[features]\n# one writer per file\nclaims = true  # keep\nsync = false\n\n[lore]\nstale_percent = 30\n";
        let edited = set_in_text(text, "claims", false);
        assert_eq!(
            edited,
            text.replace("claims = true  # keep", "claims = false  # keep")
        );
        assert_eq!(
            set_in_text(text, "sync", true),
            text.replace("sync = false", "sync = true")
        );
    }

    #[test]
    fn adds_the_key_inside_the_table_or_the_table_at_the_end() {
        let text = "spec = \"acs/0.1\"\n[features]\nclaims = false\n\n[lore]\nstale_percent = 30\n";
        assert_eq!(
            set_in_text(text, "lore", false),
            "spec = \"acs/0.1\"\n[features]\nclaims = false\nlore = false\n\n[lore]\nstale_percent = 30\n"
        );
        assert_eq!(
            set_in_text("spec = \"acs/0.1\"\r\n# [ledger]", "sync", true),
            "spec = \"acs/0.1\"\r\n# [ledger]\r\n\r\n[features]\r\nsync = true\r\n"
        );
    }

    #[test]
    fn edited_text_parses_to_the_switch_asked_for() {
        for feature in Feature::ALL {
            let text = "spec = \"acs/0.1\"\n# [ledger]\n# store_prompts = false\n";
            let off: Config = toml::from_str(&set_in_text(text, feature.key(), false)).unwrap();
            assert!(!off.features.is_on(feature));
            let on: Config = toml::from_str(&set_in_text(text, feature.key(), true)).unwrap();
            assert!(on.features.is_on(feature));
        }
    }

    #[test]
    fn the_list_fits_80_columns_and_names_every_feature() {
        let text = render(&acs_core::Features::default());
        for feature in Feature::ALL {
            assert!(text.contains(feature.key()), "{feature}");
        }
        for line in text.lines() {
            assert!(line.chars().count() <= 80, "{line:?}");
        }
    }
}
