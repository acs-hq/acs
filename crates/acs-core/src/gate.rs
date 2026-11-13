//! In-process interfaces between the gate's components.
//!
//! These are not wire formats (they have no JSON Schema and are not part of the spec); they are the
//! Rust contracts that let the components be built independently:
//!
//! - path normalization (`acs-paths`) produces [`ResolvedPath`]s,
//! - shell analysis (`acs-shell`) produces a [`ShellAnalysis`],
//! - the policy loader (`acs-policy`) implements [`PolicyView`],
//! - the state store (`acs-store`) loads a [`ClaimsSnapshot`] and applies [`ClaimRequest`]s,
//!   re-checking every request with [`claim_conflicts`] inside its transaction,
//! - the hook pipeline assembles a [`GateInput`], and the decision engine turns it into a
//!   [`GateOutput`].
//!
//! The serde derives exist for test fixtures and debugging output only.

use std::fmt;

use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use crate::decision::Decision;
use crate::event::ToolEvent;
use crate::ids::{NoticeId, OverrideId, SeatId};
use crate::policy_types::{Config, Destructive, Owner, ZoneKind};
use crate::records::{Claim, ClaimOrigin, ClaimScope, Notice, Seat};
use crate::values::RepoPath;

// ---------------------------------------------------------------------------------------------
// Paths (produced by acs-paths)
// ---------------------------------------------------------------------------------------------

/// Where a path ends up after normalization.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PathLocation {
    /// Inside the repository's working tree. The [`RepoPath`] is canonical: symlinks and junctions
    /// resolved, `/`-separated, and on case-insensitive filesystems in the on-disk case of every
    /// existing component, so two spellings of one file yield equal values.
    InRepo(RepoPath),
    /// Outside the repository root, directly or through a link. Holds the canonical absolute
    /// path.
    Outside(String),
    /// The path could not be resolved (for example an unreadable parent or a link loop).
    Unresolvable(String),
}

/// One path after normalization, with how it got there.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResolvedPath {
    /// The path exactly as it was given (absolute, or relative to the event's `cwd`).
    pub raw: String,
    /// Where it resolves to.
    pub location: PathLocation,
    /// Whether resolution passed through at least one symlink or junction.
    pub via_link: bool,
    /// For a directory that an operation would recurse into: repository paths reachable through
    /// links inside that subtree, such as a junction in a scratch copy pointing into the live
    /// tree. Empty otherwise. A recursive delete of such a directory reaches these paths.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub linked_into_repo: Vec<RepoPath>,
}

impl ResolvedPath {
    /// The repository path, if the path resolves inside the repository.
    #[must_use]
    pub fn repo_path(&self) -> Option<&RepoPath> {
        match &self.location {
            PathLocation::InRepo(path) => Some(path),
            PathLocation::Outside(_) | PathLocation::Unresolvable(_) => None,
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Shell analysis (produced by acs-shell)
// ---------------------------------------------------------------------------------------------

/// What an operation does to a target.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TargetOp {
    /// Reads the target.
    Read,
    /// Creates, overwrites or edits the target.
    Write,
    /// Removes the target. A move is a delete of its source plus a write of its destination.
    Delete,
}

/// How a simple command is classified. One command can have several classes (`mv` is
/// `move`; `curl -o f` is `network` and `write`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CommandClass {
    /// Only reads files.
    Read,
    /// Creates or modifies files.
    Write,
    /// Removes files.
    Delete,
    /// Moves or renames files.
    Move,
    /// Rewrites git history or discards work (`reset --hard`, `rebase`, `commit --amend`,
    /// `push --force`, `filter-branch`, `clean`, ...).
    GitHistoryRewrite,
    /// Talks to the network.
    Network,
    /// Effects cannot be determined (`eval`, `sh -c`, `python -c`, running a script, `xargs`,
    /// unknown programs).
    Unknown,
}

/// How sure the analysis is about a target path.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Confidence {
    /// A literal path after quote removal.
    Exact,
    /// A glob pattern; the pipeline expands it against the working tree.
    Pattern,
    /// Contains an unexpanded variable or command substitution (`"$VAR"/`, `$(...)`); the real
    /// target is unknown. Treat like unknown targets for destructive operations.
    Partial,
}

/// A path a simple command acts on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShellTarget {
    /// The path as written in the command, after quote removal; relative to the command's
    /// working directory (`cd` within the command line is tracked when it can be).
    pub path: String,
    /// What the command does to it.
    pub op: TargetOp,
    /// Whether the operation recurses into a directory (`rm -r`, `cp -r`, `find ... -delete`).
    pub recursive: bool,
    /// How sure the analysis is.
    pub confidence: Confidence,
}

/// Git-specific facts about a `git` invocation.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct GitInvocation {
    /// The subcommand, for example `push`, `reset`, `commit`.
    pub subcommand: String,
    /// Whether it rewrites history or discards work (see [`CommandClass::GitHistoryRewrite`]).
    pub rewrites_history: bool,
    /// Whether it is a force push (`--force`, `-f`, `--force-with-lease`, `+refspec`).
    pub force_push: bool,
    /// Whether it creates a commit (`commit`, `merge`, `cherry-pick`, `revert`, ...); the
    /// pipeline records a pending-commit marker.
    pub creates_commit: bool,
    /// Branches or refs named explicitly (push refspec destinations, `reset`/`rebase` targets).
    /// Empty when the command acts on the current branch implicitly.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub refs: Vec<String>,
    /// The directory git runs in, after a `cd` earlier in the command line and git's own `-C`:
    /// relative to where the command line started (empty for that directory itself) or
    /// absolute. Not meaningful when `dir_unknown` is set.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub dir: String,
    /// Whether that directory could not be determined (`cd "$X" && git ...`, `git -C "$X" ...`).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub dir_unknown: bool,
}

/// One simple command (a program and its arguments) from a command line.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SimpleCommand {
    /// The words after quote removal; `words[0]` is the program. Unexpanded parameters stay
    /// literal (`$VAR`). Ban patterns match these words joined by single spaces.
    pub words: Vec<String>,
    /// The command's classes. Never empty; `[unknown]` when nothing is known.
    pub classes: Vec<CommandClass>,
    /// The paths it acts on, including redirection targets.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub targets: Vec<ShellTarget>,
    /// Whether `targets` is believed complete. `false` means the command may touch paths not
    /// listed.
    pub targets_known: bool,
    /// Set when the program is `git`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git: Option<GitInvocation>,
}

/// The analysis of a whole shell command line.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ShellAnalysis {
    /// Every simple command in the line, in source order, across pipes, lists (`&&`, `||`, `;`),
    /// subshells and command substitutions.
    pub commands: Vec<SimpleCommand>,
    /// Whether the whole line parsed without errors. When `false`, treat the line as having
    /// unknown targets.
    pub complete: bool,
}

impl ShellAnalysis {
    /// Whether any command may delete, move or rewrite history.
    #[must_use]
    pub fn is_destructive(&self) -> bool {
        self.commands.iter().any(|c| {
            c.classes.iter().any(|class| {
                matches!(
                    class,
                    CommandClass::Delete | CommandClass::Move | CommandClass::GitHistoryRewrite
                )
            })
        })
    }

    /// Whether some target cannot be determined: the line did not parse, a command's targets are
    /// incomplete, or a target is only partially known.
    #[must_use]
    pub fn has_unknown_targets(&self) -> bool {
        !self.complete
            || self.commands.iter().any(|c| {
                !c.targets_known
                    || c.targets
                        .iter()
                        .any(|t| t.confidence == Confidence::Partial)
            })
    }
}

// ---------------------------------------------------------------------------------------------
// Policy (implemented by acs-policy)
// ---------------------------------------------------------------------------------------------

/// The zone that governs a path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ZoneMatch<'a> {
    /// The zone's kind.
    pub kind: ZoneKind,
    /// The glob that matched (the most specific one).
    pub pattern: &'a str,
    /// The owners as written (`email` or `team:<name>`), for messages.
    pub owners: &'a [Owner],
    /// The owners expanded to pilot emails (teams resolved), for membership checks.
    pub owner_pilots: &'a [String],
}

/// Compiled policy as the engine sees it.
///
/// Implementations are built for one repository with its filesystem's case sensitivity known:
/// on case-insensitive filesystems every match is case-insensitive. All path arguments are
/// repository-relative. Glob semantics are those of the spec's glob dialect.
pub trait PolicyView {
    /// `config.toml`, with defaults applied.
    fn config(&self) -> &Config;

    /// The destructive-operation limits from `guards.toml`.
    fn destructive(&self) -> &Destructive;

    /// The first `sealed` glob (built-in defaults plus `guards.toml`) matching `path`, if any.
    /// Sealed zones from `territory.toml` are reported through [`PolicyView::zone`] instead.
    fn sealed_match(&self, path: &RepoPath) -> Option<&str>;

    /// The most specific zone covering `path`, if any. No zone means `shared`.
    fn zone(&self, path: &RepoPath) -> Option<ZoneMatch<'_>>;

    /// Whether `branch` matches a `protected_branches` glob.
    fn is_protected_branch(&self, branch: &str) -> bool;

    /// `guards.toml`'s `protect_outside_repo`: whether every write or delete outside the
    /// repository asks, not only unresolvable ones and ones that link back into it.
    fn protect_outside_repo(&self) -> bool;

    /// The first `destructive.ban` pattern matching a simple command, given its words joined by
    /// single spaces.
    fn banned_match(&self, command: &str) -> Option<&str>;
}

// ---------------------------------------------------------------------------------------------
// Claims (loaded and applied by acs-store)
// ---------------------------------------------------------------------------------------------

/// Where a claim in a snapshot comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClaimSource {
    /// This machine's state database.
    Local,
    /// Another machine, through the synced remote-claims view.
    Remote,
}

/// A live claim with what the engine needs to explain it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HeldClaim {
    /// The claim.
    pub claim: Claim,
    /// Where it comes from.
    pub source: ClaimSource,
    /// The holder's harness, when known (local claims), for messages.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub holder_harness: Option<String>,
    /// When the holder was last seen, when known, for messages.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub holder_last_seen_at: Option<Timestamp>,
}

/// The claims relevant to one event: every claim (any seat, including the requesting seat's own)
/// whose scope is in a file the event touches.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ClaimsSnapshot {
    /// The claims.
    pub claims: Vec<HeldClaim>,
    /// Whether the remote-claims view is stale (last successful fetch older than
    /// `3 × sync_interval_secs`). Stale remote claims are ignored (fail open, with a warning).
    pub remote_stale: bool,
}

/// A claim the engine wants acquired for the event. The store assigns ids and timestamps and
/// acquires all requests of one event atomically, re-checking each with [`claim_conflicts`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClaimRequest {
    /// What to claim.
    pub scope: ClaimScope,
    /// The requesting seat's target branch.
    pub target_branch: String,
    /// Time to live, from `config.claim_ttl_secs`.
    pub ttl_secs: u64,
    /// Who is claiming.
    pub origin: ClaimOrigin,
}

/// Whether `existing` conflicts with `seat` claiming `scope` on `target_branch` at `now`
/// (spec section 6, claim rules). The engine and the store decide with [`blocking_claims`], which
/// adds what happens when two machines hold the same scope.
///
/// It conflicts when all hold: the scopes overlap; the holder is a different seat (the same
/// pilot's other seats count as different); the target branches are equal; the claim is not
/// expired; and it is local, or remote with a view that is not stale.
#[must_use]
pub fn claim_conflicts(
    existing: &HeldClaim,
    seat: &SeatId,
    scope: &ClaimScope,
    target_branch: &str,
    now: Timestamp,
    remote_stale: bool,
) -> bool {
    let claim = &existing.claim;
    claim.scope.overlaps(scope)
        && &claim.seat_id != seat
        && claim.target_branch == target_branch
        && !claim.is_expired(now)
        && (existing.source == ClaimSource::Local || !remote_stale)
}

/// Whether `existing`, held on another machine, gives way to `own`, held on this one (spec section
/// 6, simultaneous claims). Each machine can grant a claim before it has heard of the other's, and
/// the two then compare the same pair of claims: the one that comes first keeps the scope.
#[must_use]
pub fn gives_way(existing: &HeldClaim, own: &HeldClaim, now: Timestamp) -> bool {
    existing.source == ClaimSource::Remote
        && own.source == ClaimSource::Local
        && own.claim.scope.overlaps(&existing.claim.scope)
        && own.claim.target_branch == existing.claim.target_branch
        && !own.claim.is_expired(now)
        && own.claim.precedes(&existing.claim)
}

/// The claims in `claims` that stop `seat` from claiming `scope` on `target_branch` at `now`. An
/// empty result means it may. The single implementation of the spec section 6 rules: the engine
/// and the store MUST both use it, and `claims` must hold the seat's own claims on the file.
#[must_use]
pub fn blocking_claims<'a>(
    claims: &'a [HeldClaim],
    seat: &SeatId,
    scope: &ClaimScope,
    target_branch: &str,
    now: Timestamp,
    remote_stale: bool,
) -> Vec<&'a HeldClaim> {
    let own = claims.iter().find(|held| {
        held.source == ClaimSource::Local
            && &held.claim.seat_id == seat
            && &held.claim.scope == scope
            && held.claim.target_branch == target_branch
    });
    claims
        .iter()
        .filter(|held| claim_conflicts(held, seat, scope, target_branch, now, remote_stale))
        .filter(|held| !own.is_some_and(|own| gives_way(held, own, now)))
        .collect()
}

// ---------------------------------------------------------------------------------------------
// Engine input and output (assembled by the hook pipeline, consumed by the engine)
// ---------------------------------------------------------------------------------------------

/// A path the event acts on, resolved, with what happens to it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Target {
    /// The resolved path.
    pub path: ResolvedPath,
    /// What happens to it.
    pub op: TargetOp,
    /// Whether the operation recurses into it.
    pub recursive: bool,
    /// For recursive or glob targets: the files inside the repository the operation reaches,
    /// as listed by the pipeline. `None` when not expanded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expansion: Option<Expansion>,
}

/// The files a recursive or glob target reaches.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Expansion {
    /// The files, repository-relative.
    pub files: Vec<RepoPath>,
    /// Whether the listing stopped early (the pipeline caps it just above the largest budget it
    /// must check), so `files` is a lower bound.
    pub truncated: bool,
}

/// Gate-wide conditions for this event.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct GateState {
    /// `acs off` is active for this repository on this machine: only sealed paths, banned
    /// commands and ACS's self-protection rules are enforced.
    pub gate_off: bool,
    /// The state database could not be used (locked, corrupt or missing): destructive actions
    /// are denied, other actions allowed with a warning, and no claims are acquired.
    pub state_unavailable: bool,
}

/// The human overrides in force for the requesting seat (spec section 7, overrides). An
/// override lets a non-owner agent change files in an `owned` zone; it lifts nothing else.
///
/// The pipeline implements it over the state database, matching each override's glob with the
/// policy's glob dialect; the engine only asks which override, if any, covers a path.
pub trait OverrideView: fmt::Debug {
    /// The override in force for the seat whose glob matches `path`, if any.
    fn covering(&self, path: &RepoPath) -> Option<&OverrideId>;
}

/// An [`OverrideView`] with no overrides.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoOverrides;

impl OverrideView for NoOverrides {
    fn covering(&self, _path: &RepoPath) -> Option<&OverrideId> {
        None
    }
}

/// An override the engine relied on for one path of the event. The pipeline records the use,
/// and for a `once` override uses it up.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OverrideApplied {
    /// The override.
    pub override_id: OverrideId,
    /// The path it let through.
    pub path: RepoPath,
}

/// A location outside the repository that agents may never write or delete: ACS's own state
/// (`ACS_HOME`), git directories outside the work tree, and the user-level configuration of every
/// supported harness. The pipeline supplies them; the engine denies a write or delete that reaches
/// one with `sealed_path`, like a sealed glob, whatever the gate state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OutsideSeal {
    /// The absolute path. A target equal to it or below it is sealed. Compared after turning
    /// backslashes into `/` and ignoring ASCII case, which errs on the side of sealing.
    pub path: String,
    /// What it is, for messages (for example `ACS state` or `Claude Code user settings`).
    pub label: String,
    /// The path relative to the user's home directory, `/`-separated, with a trailing `/` for a
    /// directory (`.claude/settings.json`, `.acs/`), when it lies there. A command whose effects
    /// ACS cannot see (an interpreter, a script) is denied when one of its words mentions it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub home_relative: Option<String>,
}

impl OutsideSeal {
    /// Whether the absolute path `path` is this location or lies below it.
    #[must_use]
    pub fn covers(&self, path: &str) -> bool {
        let seal = fold_abs(&self.path);
        let seal = seal.trim_end_matches('/');
        let path = fold_abs(path);
        !seal.is_empty()
            && (path == seal
                || path
                    .strip_prefix(seal)
                    .is_some_and(|rest| rest.starts_with('/')))
    }

    /// Whether `word` (a word of a shell command) mentions this location.
    #[must_use]
    pub fn mentioned_in(&self, word: &str) -> bool {
        let word = fold_abs(word);
        let path = fold_abs(&self.path);
        if word.contains(path.trim_end_matches('/')) {
            return true;
        }
        self.home_relative.as_deref().is_some_and(|rel| {
            let rel = fold_abs(rel);
            let bare = rel.trim_end_matches('/');
            word.contains(rel.as_str()) || word.ends_with(&format!("/{bare}"))
        })
    }
}

/// An absolute path in a comparable form: `/`-separated, ASCII lowercase, without a Windows
/// verbatim prefix.
fn fold_abs(path: &str) -> String {
    let path = path.replace('\\', "/").to_ascii_lowercase();
    path.strip_prefix("//?/").map(str::to_owned).unwrap_or(path)
}

/// Everything the engine needs to decide a `pre_tool` event. Pure data: no I/O happens past this
/// point.
#[derive(Debug, Clone, Copy)]
pub struct GateInput<'a> {
    /// The canonical event.
    pub event: &'a ToolEvent,
    /// The requesting seat (pilot, branch, target branch).
    pub seat: &'a Seat,
    /// Who acts. Territory (`owned` zones) binds agents only; a human claiming through the CLI
    /// is not checked against it.
    pub origin: ClaimOrigin,
    /// Every path the event acts on: the event's `paths` for file tools (a move becomes a delete
    /// of the source and a write of the destination), or the shell analysis targets for shell
    /// commands; resolved and, where needed, expanded.
    pub targets: &'a [Target],
    /// The shell analysis, when the tool is a shell command.
    pub shell: Option<&'a ShellAnalysis>,
    /// The claims relevant to `targets`.
    pub claims: &'a ClaimsSnapshot,
    /// Undelivered notices for this seat.
    pub notices: &'a [Notice],
    /// The human overrides in force for this seat.
    pub overrides: &'a dyn OverrideView,
    /// Locations outside the repository agents may never write or delete.
    pub outside_sealed: &'a [OutsideSeal],
    /// Gate-wide conditions.
    pub state: GateState,
    /// The current time.
    pub now: Timestamp,
}

/// A pre-image the pipeline must capture before the tool runs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SnapshotRequest {
    /// The file or directory.
    pub path: RepoPath,
    /// Whether to capture a directory's whole subtree.
    pub recursive: bool,
}

/// What the engine returns for a `pre_tool` event.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GateOutput {
    /// The decision. `claims_acquired` is left empty; the pipeline fills it after the store
    /// acquires `claim_requests`.
    pub decision: Decision,
    /// Claims to acquire. Empty when the outcome is `deny`.
    pub claim_requests: Vec<ClaimRequest>,
    /// Pre-images to capture. Empty when the outcome is `deny`.
    pub snapshot_requests: Vec<SnapshotRequest>,
    /// Notices whose bodies were put in `decision.context`, to mark delivered.
    pub delivered_notices: Vec<NoticeId>,
    /// Overrides the decision relied on. Empty when the outcome is `deny`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub overrides_applied: Vec<OverrideApplied>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn held(seat: &str, scope: ClaimScope, branch: &str, source: ClaimSource) -> HeldClaim {
        let t: Timestamp = "2026-10-29T12:00:00Z".parse().unwrap();
        HeldClaim {
            claim: Claim {
                claim_id: "clm_1".parse().unwrap(),
                seat_id: seat.parse().unwrap(),
                pilot: "sara@example.com".into(),
                scope,
                target_branch: branch.into(),
                acquired_at: t,
                heartbeat_at: t,
                ttl_secs: 900,
                origin: ClaimOrigin::Agent,
            },
            source,
            holder_harness: None,
            holder_last_seen_at: None,
        }
    }

    fn file(path: &str) -> ClaimScope {
        ClaimScope::File {
            file: path.parse().unwrap(),
        }
    }

    #[test]
    fn claim_conflict_rules() {
        let me: SeatId = "seat_me".parse().unwrap();
        let soon: Timestamp = "2026-10-29T12:05:00Z".parse().unwrap();
        let late: Timestamp = "2026-10-29T12:15:00Z".parse().unwrap();
        let local = held("seat_b", file("a.rs"), "main", ClaimSource::Local);
        let remote = held("seat_b", file("a.rs"), "main", ClaimSource::Remote);
        let a = file("a.rs");

        assert!(claim_conflicts(&local, &me, &a, "main", soon, false));
        assert!(!claim_conflicts(
            &local,
            &me,
            &file("b.rs"),
            "main",
            soon,
            false
        ));
        assert!(!claim_conflicts(&local, &me, &a, "experiment", soon, false));
        assert!(
            !claim_conflicts(&local, &me, &a, "main", late, false),
            "expired"
        );
        assert!(
            claim_conflicts(&local, &me, &a, "main", soon, true),
            "local ignores staleness"
        );
        assert!(claim_conflicts(&remote, &me, &a, "main", soon, false));
        assert!(
            !claim_conflicts(&remote, &me, &a, "main", soon, true),
            "stale remote fails open"
        );
        let own = held("seat_me", file("a.rs"), "main", ClaimSource::Local);
        assert!(
            !claim_conflicts(&own, &me, &a, "main", soon, false),
            "own seat"
        );
    }

    /// `seat`'s claim on `a.rs`, taken `after` seconds past the base time of [`held`].
    fn timed(seat: &str, id: &str, after: i64, source: ClaimSource) -> HeldClaim {
        let mut claim = held(seat, file("a.rs"), "main", source);
        claim.claim.claim_id = id.parse().unwrap();
        let start = claim.claim.acquired_at.as_second();
        claim.claim.acquired_at = Timestamp::from_second(start + after).unwrap();
        claim.claim.heartbeat_at = claim.claim.acquired_at;
        claim
    }

    fn blocked(view: &[HeldClaim], seat: &str, now: Timestamp, stale: bool) -> bool {
        let seat: SeatId = seat.parse().unwrap();
        !blocking_claims(view, &seat, &file("a.rs"), "main", now, stale).is_empty()
    }

    #[test]
    fn of_two_machines_holding_one_file_exactly_one_gives_way() {
        use ClaimSource::{Local, Remote};
        let now: Timestamp = "2026-10-29T12:05:00Z".parse().unwrap();
        for (a_after, b_after) in [(0, 0), (0, 1), (1, 0)] {
            for (a_id, b_id) in [("clm_a", "clm_b"), ("clm_b", "clm_a")] {
                let a_view = [
                    timed("seat_a", a_id, a_after, Local),
                    timed("seat_b", b_id, b_after, Remote),
                ];
                let b_view = [
                    timed("seat_b", b_id, b_after, Local),
                    timed("seat_a", a_id, a_after, Remote),
                ];
                let a_blocked = blocked(&a_view, "seat_a", now, false);
                let b_blocked = blocked(&b_view, "seat_b", now, false);
                assert_ne!(a_blocked, b_blocked, "{a_after} {b_after} {a_id} {b_id}");
                assert_eq!(
                    a_blocked,
                    (b_after, b_id) < (a_after, a_id),
                    "the earlier claim keeps the file: {a_after} {b_after} {a_id} {b_id}"
                );
            }
        }
    }

    #[test]
    fn only_a_live_earlier_claim_on_this_machine_outranks_a_claim() {
        use ClaimSource::{Local, Remote};
        let now: Timestamp = "2026-10-29T12:05:00Z".parse().unwrap();
        let earlier = timed("seat_me", "clm_a", 0, Local);
        let later = timed("seat_far", "clm_b", 60, Remote);

        assert!(!blocked(
            &[earlier.clone(), later.clone()],
            "seat_me",
            now,
            false
        ));
        assert!(
            blocked(std::slice::from_ref(&later), "seat_me", now, false),
            "holding nothing"
        );
        assert!(
            blocked(&[earlier.clone(), later.clone()], "seat_other", now, false),
            "a third seat is blocked either way"
        );

        let mut same_machine = later.clone();
        same_machine.source = Local;
        assert!(
            blocked(&[earlier.clone(), same_machine], "seat_me", now, false),
            "one machine never needs the tie-break: its claims are atomic"
        );

        let mut other_branch = earlier.clone();
        other_branch.claim.target_branch = "experiment".into();
        assert!(blocked(
            &[other_branch, later.clone()],
            "seat_me",
            now,
            false
        ));

        let past_ttl: Timestamp = "2026-10-29T12:15:10Z".parse().unwrap();
        assert!(
            blocked(&[earlier, later], "seat_me", past_ttl, false),
            "an expired claim of ours outranks nothing"
        );
    }

    #[test]
    fn shell_analysis_flags() {
        let cmd = |classes: Vec<CommandClass>, known: bool, confidence: Confidence| SimpleCommand {
            words: vec!["x".into()],
            classes,
            targets: vec![ShellTarget {
                path: "p".into(),
                op: TargetOp::Delete,
                recursive: false,
                confidence,
            }],
            targets_known: known,
            git: None,
        };
        let read = ShellAnalysis {
            commands: vec![cmd(vec![CommandClass::Read], true, Confidence::Exact)],
            complete: true,
        };
        assert!(!read.is_destructive() && !read.has_unknown_targets());
        let rm_var = ShellAnalysis {
            commands: vec![cmd(vec![CommandClass::Delete], true, Confidence::Partial)],
            complete: true,
        };
        assert!(rm_var.is_destructive() && rm_var.has_unknown_targets());
        let broken = ShellAnalysis {
            complete: false,
            ..read
        };
        assert!(broken.has_unknown_targets());
    }
}
