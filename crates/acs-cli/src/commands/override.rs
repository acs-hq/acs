//! `acs override`: a human lets agents change files in `owned` zones they do not own (WP7.1,
//! decision 0026).
//!
//! Granting needs a person at a terminal: standard input and standard error must be terminals,
//! and the person types back a random code. Agents are also denied the command by the hook
//! (decision 0026), so the terminal check is the second line, for agents whose shell ACS does
//! not see. `docs/security/overrides.md` lists what it does not stop.

use std::io::{BufRead, IsTerminal, Write};
use std::process::ExitCode;

use acs_core::{OverrideId, OverrideMode, Timestamp, ZoneKind};
use acs_policy::glob::Glob;
use acs_store::{FLAG_GATE_OFF, NewOverride};
use clap::Args;
use serde_json::json;

use super::support::{CliError, OutputArgs, finish, open_repo, print_json, short_duration, wrap};
use crate::runtime::{OpError, RepoContext};

/// Letters of the confirmation code: no `I` or `O`, which read like digits.
const CODE_LETTERS: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZ";

/// Arguments of `acs override`.
#[derive(Debug, Args)]
#[command(after_help = "Examples:\n  \
    acs override --path 'payments/**' --once     one tool call, by the first agent that needs it\n  \
    acs override --path 'payments/refund.ts' --session   that agent's whole session\n  \
    acs override --revoke ovr_...                withdraw an override not used up yet\n\n\
    Only `owned` zones are affected: claims and sealed paths still hold. An override nobody uses \
    lapses after claim_ttl_secs (15 minutes by default). Run it in your own terminal: it asks you \
    to type a code, and agents are not allowed to run it.")]
pub struct OverrideArgs {
    /// The files to let agents change, as a glob relative to the repository root.
    #[arg(long, value_name = "GLOB", required_unless_present = "revoke")]
    pub path: Option<String>,
    /// For one tool call: the first agent call that needs it uses it up.
    #[arg(long, conflicts_with_all = ["session", "revoke"])]
    pub once: bool,
    /// For the rest of the session of the first agent that uses it.
    #[arg(long, conflicts_with = "revoke")]
    pub session: bool,
    /// Withdraw an override that has not been used up.
    #[arg(long, value_name = "ID", conflicts_with = "path")]
    pub revoke: Option<String>,
    /// Output format.
    #[command(flatten)]
    pub output: OutputArgs,
}

/// Runs `acs override`.
#[must_use]
pub fn run(args: &OverrideArgs) -> ExitCode {
    let json = args.output.json;
    let result = match (&args.revoke, &args.path) {
        (Some(id), _) => revoke(id, json),
        (None, Some(path)) => grant(args, path, json),
        (None, None) => Err(CliError::new(
            "invalid",
            "Name the files with --path <glob>.",
        )),
    };
    finish(json, result)
}

fn grant(args: &OverrideArgs, raw: &str, json: bool) -> Result<ExitCode, CliError> {
    let mode = match (args.once, args.session) {
        (true, false) => OverrideMode::Once,
        (false, true) => OverrideMode::Session,
        _ => {
            return Err(CliError::new(
                "invalid",
                "Say how long the override lasts: --once (one tool call) or --session (the rest \
                 of that agent's session).",
            ));
        }
    };
    let (ctx, _cwd) = open_repo()?;
    let pattern = normalize_pattern(raw);
    Glob::new(&pattern, ctx.resolver.case_insensitive()).map_err(|err| {
        CliError::new(
            "invalid",
            format!("`{raw}` is not a valid glob: {err}. Use a path relative to the repository root, such as `payments/**`."),
        )
    })?;
    let db = ctx
        .db()
        .map_err(|why| CliError::from(OpError::Unavailable(why.to_owned())))?;

    if !(std::io::stdin().is_terminal() && std::io::stderr().is_terminal()) {
        return Err(CliError::new(
            "not_interactive",
            "acs override must be run by a person in a terminal: it lets agents into owned \
             territory, so it asks you to type a code. Run it in your own terminal, not through \
             an agent or a pipe.",
        ));
    }
    let code = confirmation_code()?;
    let mut stderr = std::io::stderr();
    let _ = write!(
        stderr,
        "{}",
        wrap(
            &format!(
                "This lets agents change files matching `{pattern}` in owned territory, {}. \
                 Claims and sealed paths still apply.",
                lasts(mode)
            ),
            "",
            ""
        )
    );
    let mut stdin = std::io::stdin().lock();
    if !confirm(&code, &mut stdin, &mut stderr) {
        return Err(CliError::new(
            "cancelled",
            "The code did not match; nothing was granted.",
        ));
    }

    let now = Timestamp::now();
    let ttl = ctx.policy.policy.files().config.claim_ttl_secs.get();
    let expires_at = Timestamp::from_second(
        now.as_second()
            .saturating_add(i64::try_from(ttl).unwrap_or(i64::MAX)),
    )
    .unwrap_or(Timestamp::MAX);
    let granted_by = acs_git::repo::pilot_email_or_fallback(&ctx.layout.work_tree);
    let id = db
        .grant_override(&NewOverride {
            pattern: &pattern,
            mode,
            granted_by: &granted_by,
            now,
            expires_at,
        })
        .map_err(OpError::from)?;
    let warnings = warnings(&ctx);

    if json {
        print_json(&json!({
            "ok": true,
            "override_id": id.as_str(),
            "pattern": pattern,
            "mode": mode,
            "granted_by": granted_by,
            "expires_at": expires_at.to_string(),
            "warnings": warnings,
        }));
    } else {
        println!("Override {id} granted for `{pattern}`.");
        print!(
            "{}",
            wrap(
                &format!(
                    "Agents may change matching files in owned territory {}. If no agent uses it \
                     within {}, it lapses. `acs status` lists it; `acs override --revoke {id}` \
                     withdraws it.",
                    lasts(mode),
                    short_duration(i64::try_from(ttl).unwrap_or(i64::MAX))
                ),
                "",
                ""
            )
        );
        for warning in &warnings {
            print!("{}", wrap(warning, "warning: ", "         "));
        }
    }
    Ok(ExitCode::SUCCESS)
}

fn revoke(raw: &str, json: bool) -> Result<ExitCode, CliError> {
    let id: OverrideId = raw.parse().map_err(|_| {
        CliError::new(
            "invalid",
            format!(
                "`{raw}` is not an override id (they start with `ovr_`; `acs status` lists them)."
            ),
        )
    })?;
    let (ctx, _cwd) = open_repo()?;
    let db = ctx
        .db()
        .map_err(|why| CliError::from(OpError::Unavailable(why.to_owned())))?;
    if db.override_by_id(&id).map_err(OpError::from)?.is_none() {
        return Err(CliError::new(
            "not_found",
            format!("There is no override {id} in this repository."),
        ));
    }
    let revoked = db
        .revoke_override(&id, Timestamp::now())
        .map_err(OpError::from)?;
    if json {
        print_json(&json!({"ok": true, "override_id": id.as_str(), "revoked": revoked}));
    } else if revoked {
        println!("Override {id} withdrawn.");
    } else {
        println!("Override {id} was already used up or withdrawn.");
    }
    Ok(ExitCode::SUCCESS)
}

/// How long an override lasts, as a phrase.
fn lasts(mode: OverrideMode) -> &'static str {
    match mode {
        OverrideMode::Once => "for the next tool call that needs it",
        OverrideMode::Session => "for the rest of the session of the first agent that uses it",
    }
}

/// What the person should know: the gate is off, or no zone is owned at all.
fn warnings(ctx: &RepoContext) -> Vec<String> {
    let mut out = Vec::new();
    if ctx
        .db()
        .ok()
        .and_then(|db| db.flag_is_set(FLAG_GATE_OFF).ok())
        .unwrap_or(false)
    {
        out.push("The gate is off (`acs off`), so territory is not enforced right now.".to_owned());
    }
    let owned = ctx
        .policy
        .policy
        .files()
        .territory
        .zone
        .iter()
        .any(|zone| zone.kind == ZoneKind::Owned);
    if !owned {
        out.push(
            "No zone in .acs/policy/territory.toml is `owned`, so this override changes nothing."
                .to_owned(),
        );
    }
    out
}

/// A glob as typed, made repository-relative: `./` dropped, backslashes turned into `/`.
fn normalize_pattern(raw: &str) -> String {
    let slashed = raw.trim().replace('\\', "/");
    slashed
        .strip_prefix("./")
        .map_or(slashed.clone(), str::to_owned)
}

/// Four random letters from the operating system's generator.
fn confirmation_code() -> Result<String, CliError> {
    let mut bytes = [0u8; 4];
    getrandom::fill(&mut bytes).map_err(|err| {
        CliError::new("random", format!("cannot make a confirmation code: {err}"))
    })?;
    Ok(bytes
        .iter()
        .map(|b| char::from(CODE_LETTERS[usize::from(*b) % CODE_LETTERS.len()]))
        .collect())
}

/// Asks for `code` on `prompt` and reads one line from `input`. Case and surrounding spaces are
/// ignored.
fn confirm(code: &str, input: &mut impl BufRead, prompt: &mut impl Write) -> bool {
    let _ = write!(prompt, "Type {code} to confirm: ");
    let _ = prompt.flush();
    let mut line = String::new();
    if input.read_line(&mut line).is_err() {
        return false;
    }
    line.trim().eq_ignore_ascii_case(code)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_exact_code_confirms() {
        let mut prompt = Vec::new();
        assert!(confirm("ABCD", &mut "abcd\n".as_bytes(), &mut prompt));
        assert!(String::from_utf8(prompt).unwrap().contains("Type ABCD"));
        for answer in ["yes\n", "y\n", "\n", "", "ABC\n", "ABCDE\n"] {
            assert!(
                !confirm("ABCD", &mut answer.as_bytes(), &mut Vec::new()),
                "{answer:?}"
            );
        }
    }

    #[test]
    fn codes_are_four_unambiguous_letters() {
        let code = confirmation_code().unwrap();
        assert_eq!(code.len(), 4);
        assert!(code.bytes().all(|b| CODE_LETTERS.contains(&b)));
    }

    #[test]
    fn patterns_are_made_repository_relative() {
        assert_eq!(normalize_pattern("./payments/**"), "payments/**");
        assert_eq!(
            normalize_pattern("payments\\refund.ts"),
            "payments/refund.ts"
        );
        assert_eq!(normalize_pattern(" lib/** "), "lib/**");
    }
}
