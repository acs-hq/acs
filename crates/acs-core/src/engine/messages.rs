//! The agent-facing message of each reason code.
//!
//! Every message says what is blocked, by whom or by which rule, and what to do next, and never
//! suggests getting around ACS. They are deterministic: the same input gives the same text.

use jiff::Timestamp;

use super::plan::Sealed;
use super::protect::Breach;
use crate::gate::{ClaimSource, HeldClaim, ResolvedPath};
use crate::policy_types::Owner;
use crate::values::RepoPath;

/// How many paths a message names before it says "and N more".
const SHOWN: usize = 3;
/// How many holders of one path a message names.
const HOLDERS_SHOWN: usize = 2;
/// How many characters of a command a message quotes.
const COMMAND_SHOWN: usize = 120;

/// Joins up to [`SHOWN`] items as prose, then counts the rest.
fn quoted(items: &[String], total: usize) -> String {
    let more = total.saturating_sub(items.len());
    match (items, more) {
        ([], _) => String::new(),
        ([one], 0) => one.clone(),
        (items, 0) => format!(
            "{} and {}",
            items[..items.len() - 1].join(", "),
            items[items.len() - 1]
        ),
        (items, more) => format!("{} and {more} more", items.join(", ")),
    }
}

/// Collects the first [`SHOWN`] entries of a list and counts the rest, without formatting more
/// than it shows.
pub(super) struct Shown {
    items: Vec<String>,
    total: usize,
}

impl Shown {
    pub(super) fn new() -> Self {
        Self {
            items: Vec::new(),
            total: 0,
        }
    }

    pub(super) fn push(&mut self, item: impl FnOnce() -> String) {
        if self.items.len() < SHOWN {
            self.items.push(item());
        }
        self.total += 1;
    }

    pub(super) fn total(&self) -> usize {
        self.total
    }

    pub(super) fn is_empty(&self) -> bool {
        self.total == 0
    }

    fn render(&self) -> String {
        quoted(&self.items, self.total)
    }
}

fn tick(text: impl std::fmt::Display) -> String {
    format!("`{text}`")
}

fn truncate(text: &str, max: usize) -> String {
    match text.char_indices().nth(max) {
        Some((end, _)) => format!("{}...", &text[..end]),
        None => text.to_owned(),
    }
}

/// `path` with the sealing rule, for a [`Shown`] list.
pub(super) fn sealed_item(path: &RepoPath, sealed: Sealed<'_>) -> String {
    if sealed.zone {
        format!("`{path}` (sealed zone `{}`)", sealed.pattern)
    } else {
        format!("`{path}` (sealed by `{}`)", sealed.pattern)
    }
}

pub(super) fn sealed_path(list: &Shown) -> String {
    let (subject, pronoun) = if list.total == 1 {
        ("is", "it")
    } else {
        ("are", "them")
    };
    format!(
        "{} {subject} protected by ACS: agents may not change {pronoun}, so a human must. \
         Continue with other files, or ask your pilot to make this change.",
        list.render()
    )
}

pub(super) fn banned_command(command: &str, pattern: &str) -> String {
    format!(
        "The command {} is banned by this repository's policy (it matches {}). \
         Use a safer alternative, or ask your pilot to run it themselves.",
        tick(truncate(command, COMMAND_SHOWN)),
        tick(pattern)
    )
}

/// A target outside the repository that ACS protects, for a [`Shown`] list.
pub(super) fn outside_sealed_item(raw: &str, label: &str) -> String {
    format!("`{raw}` ({label})")
}

/// A command with unknown effects that mentions a protected location.
pub(super) fn sealed_mention(command: &str, what: &str) -> String {
    format!(
        "The command {} may change {what}, which is protected by ACS, and ACS cannot see what it \
         does. Agents may not change it, so a human must. Continue with other work, or ask your \
         pilot to make this change.",
        tick(truncate(command, COMMAND_SHOWN))
    )
}

/// A command that breaks one of ACS's self-protection rules.
pub(super) fn self_protection(breach: &Breach) -> String {
    match breach {
        Breach::AcsCli(shown) => format!(
            "Agents may not run {}: it changes ACS state as your pilot, not as you. To claim or \
             release your own files use the ACS tools (`acs_claim`, `acs_release`); for \
             anything else, ask your pilot to run it in their own terminal.",
            tick(shown)
        ),
        Breach::GitHooks(shown) => format!(
            "Agents may not run {}: it skips the git hooks that check this repository's claims \
             and sealed paths. Run the command without it; if a hook fails, fix the cause or \
             ask your pilot.",
            tick(shown)
        ),
        Breach::Harness(shown) => format!(
            "Agents may not run {}: it starts an agent with its hooks or the ACS plugin turned \
             off, so ACS could not protect the repository from it. Run it without that option, \
             or ask your pilot.",
            tick(shown)
        ),
    }
}

/// Paths in `owned` zones the seat's pilot does not own, each with its zone, for a [`Shown`]
/// list.
pub(super) fn owned_item(path: &RepoPath, pattern: &str, owners: &[Owner]) -> String {
    let names: Vec<String> = owners.iter().map(ToString::to_string).collect();
    let owners = if names.is_empty() {
        "nobody".to_owned()
    } else {
        quoted(&names[..names.len().min(HOLDERS_SHOWN)], names.len())
    };
    format!("`{path}` (zone `{pattern}`, owned by {owners})")
}

pub(super) fn owned_territory(list: &Shown, pattern: &str) -> String {
    let (subject, pronoun) = if list.total == 1 {
        ("is", "it")
    } else {
        ("are", "them")
    };
    format!(
        "{} {subject} owned territory: only the owners' agents may change {pronoun}. Work on \
         other files, or ask your pilot to coordinate with an owner or to let you in from \
         their own terminal (`acs override --path '{pattern}' --once`).",
        list.render()
    )
}

pub(super) fn protected_branch(subcommand: &str, branch: &str, force_push: bool) -> String {
    let what = if force_push {
        format!("A force push with `git {subcommand}`")
    } else {
        format!("`git {subcommand}`, which rewrites history,")
    };
    format!(
        "{what} would change protected branch {}. History rewrites and force pushes are blocked \
         there. Work on a feature branch, or ask your pilot to do this themselves.",
        tick(branch)
    )
}

pub(super) fn path_escapes_repo(escapes: &[&ResolvedPath]) -> String {
    let mut shown = Shown::new();
    for escape in escapes {
        shown.push(|| {
            if escape.linked_into_repo.is_empty() {
                tick(&escape.raw)
            } else {
                let mut linked = Shown::new();
                for path in &escape.linked_into_repo {
                    linked.push(|| tick(path));
                }
                format!(
                    "`{}` (it links into the repository and reaches {})",
                    escape.raw,
                    linked.render()
                )
            }
        });
    }
    format!(
        "{} {} outside the repository, where ACS cannot protect other people's work. \
         Your pilot is asked to approve it; keep changes inside the repository if you can.",
        shown.render(),
        if shown.total == 1 { "is" } else { "are" }
    )
}

pub(super) fn delete_outside_claims(list: &Shown, limit: u32) -> String {
    format!(
        "This would delete {} that you have not claimed ({}), and the limit is {limit}. \
         Only delete files you have claimed, or ask your pilot to run the delete.",
        files(list.total),
        list.render()
    )
}

fn files(count: usize) -> String {
    if count == 1 {
        "1 file".to_owned()
    } else {
        format!("{count} files")
    }
}

pub(super) fn destructive_budget(count: usize, limit: u32, lower_bound: bool) -> String {
    format!(
        "This command would delete {}{count} files, more than the limit of {limit} per command. \
         Your pilot is asked to approve it; delete in smaller steps if you can.",
        if lower_bound { "at least " } else { "" }
    )
}

pub(super) fn destructive_unknown_targets() -> String {
    "This command deletes, moves or rewrites files, but ACS cannot tell which ones (a variable, \
     substitution or script hides the targets). Your pilot is asked to approve it; spell out the \
     exact paths to avoid the question."
        .to_owned()
}

pub(super) fn state_unavailable_denied() -> String {
    "ACS cannot read its state right now, so destructive actions (deleting or moving files, \
     rewriting history) are blocked. Ordinary edits still work. Ask your pilot to run \
     `acs doctor`."
        .to_owned()
}

pub(super) fn state_unavailable_warning() -> String {
    "ACS state is unavailable: claims are not being checked or taken. Ask your pilot to run \
     `acs doctor`."
        .to_owned()
}

pub(super) fn remote_stale_warning() -> String {
    "The shared claims view is out of date, so claims held on other machines were not checked; \
     claims on this machine are still enforced."
        .to_owned()
}

/// How long ago `since` was, in the coarsest unit that is at least 1.
fn age(now: Timestamp, since: Timestamp) -> String {
    let secs = now.duration_since(since).as_secs().max(0);
    match secs {
        0..60 => "just now".to_owned(),
        60..3600 => format!("{} min ago", secs / 60),
        3600..86_400 => format!("{} h ago", secs / 3600),
        _ => format!("{} d ago", secs / 86_400),
    }
}

fn holder(held: &HeldClaim, now: Timestamp) -> String {
    let place = match (&held.holder_harness, held.source) {
        (Some(harness), _) => harness.as_str(),
        (None, ClaimSource::Remote) => "another machine",
        (None, ClaimSource::Local) => "this machine",
    };
    let seen = held.holder_last_seen_at.unwrap_or(held.claim.heartbeat_at);
    format!("{} ({place}, active {})", held.claim.pilot, age(now, seen))
}

/// The paths that conflict, each with the claims that block it.
pub(super) fn claim_conflict(conflicts: &[(&RepoPath, Vec<&HeldClaim>)], now: Timestamp) -> String {
    let mut message = String::new();
    for (path, holders) in conflicts.iter().take(SHOWN) {
        if !message.is_empty() {
            message.push(' ');
        }
        let mut names: Vec<String> = Vec::new();
        let mut seats: Vec<&str> = Vec::new();
        for held in holders {
            let seat = held.claim.seat_id.as_str();
            if !seats.contains(&seat) {
                seats.push(seat);
                if names.len() < HOLDERS_SHOWN {
                    names.push(holder(held, now));
                }
            }
        }
        message.push_str(&format!(
            "{} is claimed by {}.",
            tick(path),
            quoted(&names, seats.len())
        ));
    }
    if conflicts.len() > SHOWN {
        message.push_str(&format!(
            " {} more file(s) are claimed by others.",
            conflicts.len() - SHOWN
        ));
    }
    message.push_str(" Work on other files, or ask your pilot to coordinate.");
    message
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ages_use_the_coarsest_unit() {
        let now = Timestamp::from_second(1_800_000_000).unwrap();
        let at = |secs: i64| age(now, Timestamp::from_second(1_800_000_000 - secs).unwrap());
        assert_eq!(at(-5), "just now", "clock skew never shows a negative age");
        assert_eq!(at(0), "just now");
        assert_eq!(at(59), "just now");
        assert_eq!(at(60), "1 min ago");
        assert_eq!(at(3599), "59 min ago");
        assert_eq!(at(3600), "1 h ago");
        assert_eq!(at(86_399), "23 h ago");
        assert_eq!(at(86_400), "1 d ago");
        assert_eq!(at(3 * 86_400 + 5), "3 d ago");
    }

    #[test]
    fn lists_name_the_first_few_and_count_the_rest() {
        let items: Vec<String> = ["`a`", "`b`", "`c`"].map(String::from).to_vec();
        assert_eq!(quoted(&items[..1], 1), "`a`");
        assert_eq!(quoted(&items[..2], 2), "`a` and `b`");
        assert_eq!(quoted(&items, 3), "`a`, `b` and `c`");
        assert_eq!(quoted(&items, 9), "`a`, `b`, `c` and 6 more");
    }

    #[test]
    fn truncation_respects_character_boundaries() {
        assert_eq!(truncate("abc", 3), "abc");
        assert_eq!(truncate("abcd", 3), "abc...");
        assert_eq!(truncate("ééééé", 2), "éé...");
    }
}
