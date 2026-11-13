//! Lore injection (WP6.1, decision 0029): the first time a seat works in an entry's scope, the
//! entry is added to the context of that tool call, then never again in the session.
//!
//! "Works in" means any tool call whose paths fall in the scope (a read, a write, a claim), not
//! only the first claim: agents read before they edit, and lore that arrives with the first read
//! is there when the edit is planned. Entries are taken most specific first within
//! `[lore] max_inject_chars`; what does not fit waits for the next call in the scope. Stale
//! entries carry a request to verify them. This runs in the pipeline, not in the pure engine,
//! because it reads files and git.

use std::path::Path;

use acs_core::gate::{Confidence, PathLocation, TargetOp};
use acs_core::{LoreId, RepoPath, SeatId, Timestamp, ToolEvent, ToolKind};
use acs_lore::propose::Draft;
use acs_lore::resolve::{Served, render};
use acs_lore::{
    LORE_DIR, LoreSet, Proposal, ProposalStore, Proposer, StaleCache, Staleness, resolve,
};
use acs_store::SeatInfo;

use super::paths::Normalized;
use super::{OpError, RepoContext};

/// The most paths of one event looked up.
const MAX_PATHS: usize = 64;

/// Lore to show `seat` for a tool call on `paths` (repository-relative), already marked as
/// shown. Best effort: no lore, no state database or a git failure means no lore, never an error.
#[must_use]
pub fn inject(ctx: &RepoContext, seat: &SeatId, paths: &[&str], now: Timestamp) -> Vec<String> {
    let work_tree = &ctx.layout.work_tree;
    if !ctx.features().lore || paths.is_empty() || !work_tree.join(LORE_DIR).is_dir() {
        return Vec::new();
    }
    let Ok(db) = ctx.db() else {
        return Vec::new();
    };
    let ci = ctx.resolver.case_insensitive();
    let set = LoreSet::load(work_tree, ci);
    let paths = &paths[..paths.len().min(MAX_PATHS)];
    let matches = resolve(&set, paths);
    if matches.is_empty() {
        return Vec::new();
    }
    let Ok(seen) = db.lore_loaded_ids(seat) else {
        return Vec::new();
    };
    let candidates: Vec<_> = matches
        .iter()
        .filter(|m| !seen.contains(m.entry.id().as_str()))
        .map(|m| m.entry)
        .collect();
    if candidates.is_empty() {
        return Vec::new();
    }

    let config = &ctx.policy.policy.files().config.lore;
    let staleness = measure_all(ctx, work_tree, &candidates, config.stale_percent, ci);
    let served: Vec<Served<'_>> = candidates
        .iter()
        .zip(&staleness)
        .map(|(entry, staleness)| Served {
            entry,
            staleness: staleness.as_ref(),
        })
        .collect();
    let cap = usize::try_from(config.max_inject_chars.get()).unwrap_or(usize::MAX);
    let rendered = render(&served, cap);

    let marks: Vec<(LoreId, bool)> = rendered
        .shown
        .iter()
        .map(|&i| {
            let stale = staleness[i].as_ref().is_some_and(Staleness::is_stale);
            (candidates[i].id().clone(), stale)
        })
        .collect();
    let Ok(fresh) = db.mark_lore_loaded(seat, &marks, now) else {
        return Vec::new();
    };
    let mut blocks: Vec<String> = rendered
        .shown
        .iter()
        .zip(rendered.blocks)
        .filter(|(i, _)| fresh.contains(candidates[**i].id()))
        .map(|(_, block)| block)
        .collect();
    if !rendered.deferred.is_empty()
        && let Some(last) = blocks.last_mut()
    {
        last.push_str(&format!(
            "\n[{} more lore entr{} here did not fit; {} will come with your next tool call in \
             this scope, or run `acs lore show <path>`.]",
            rendered.deferred.len(),
            if rendered.deferred.len() == 1 {
                "y applies"
            } else {
                "ies apply"
            },
            if rendered.deferred.len() == 1 {
                "it"
            } else {
                "they"
            },
        ));
    }
    blocks
}

/// The pending proposals of the repository (`<repo home>/pending/lore`).
///
/// # Errors
///
/// [`OpError::Unavailable`] when the repository's state directory is unknown.
pub fn proposals(ctx: &RepoContext) -> Result<ProposalStore, OpError> {
    ctx.repo_home
        .as_ref()
        .map(|repo| ProposalStore::new(repo.pending_dir().join("lore")))
        .ok_or_else(|| {
            OpError::Unavailable("the repository's state directory is unavailable".to_owned())
        })
}

/// A stored proposal and who should review it.
#[derive(Debug, Clone)]
pub struct Proposed {
    /// The proposal as stored.
    pub proposal: Proposal,
    /// The replaced entry's owner, or the owners of the zone the scope falls in, when known.
    pub reviewer: Option<String>,
}

/// Checks `draft` and stores it as a proposal from `seat` for the owner's review.
///
/// # Errors
///
/// [`OpError::Invalid`] with guidance for the agent when the draft does not pass the checks or
/// the store is full; [`OpError::Unavailable`] without a state directory.
pub fn propose(
    ctx: &RepoContext,
    seat: &SeatInfo,
    draft: &Draft,
    now: Timestamp,
) -> Result<Proposed, OpError> {
    if !ctx.features().lore {
        return Err(OpError::Invalid(
            "Lore is switched off in this repository (`[features] lore` in \
             `.acs/policy/config.toml`), so the proposal was not stored."
                .to_owned(),
        ));
    }
    let ci = ctx.resolver.case_insensitive();
    let set = LoreSet::load(&ctx.layout.work_tree, ci);
    let checked = acs_lore::propose::check(draft, &set, ci).map_err(OpError::Invalid)?;
    let store = proposals(ctx)?;
    let proposal = Proposal {
        proposal_id: new_proposal_id()?,
        replaces: checked.replaces,
        scope: checked.scope,
        body: checked.body,
        why: checked.why,
        proposed_by: Proposer {
            pilot: seat.seat.pilot.clone(),
            harness: seat.seat.harness.clone(),
            session_id: seat.session_id.to_string(),
        },
        proposed_at: now,
        head: acs_git::lore::head_commit(&ctx.layout.work_tree)
            .ok()
            .flatten(),
    };
    store.save(&proposal).map_err(OpError::Invalid)?;
    let reviewer = reviewer_of(ctx, &set, &proposal);
    Ok(Proposed { proposal, reviewer })
}

/// Who should review `proposal`: the replaced entry's owner, else the owners of the zone its
/// first scope glob's literal start falls in.
#[must_use]
pub fn reviewer_of(ctx: &RepoContext, set: &LoreSet, proposal: &Proposal) -> Option<String> {
    if let Some(entry) = proposal.replaces.as_ref().and_then(|id| set.get(id)) {
        return Some(entry.front.owner.to_string());
    }
    let owners = zone_owners(ctx, proposal.scope.first()?)?;
    Some(owners.join(", "))
}

/// The owners of the zone the literal start of `glob` falls in (`payments/**` probes
/// `payments`), when that zone lists any.
#[must_use]
pub fn zone_owners(ctx: &RepoContext, glob: &str) -> Option<Vec<String>> {
    let literal: Vec<&str> = glob
        .split('/')
        .take_while(|seg| !seg.contains(['*', '?', '[', '{']))
        .collect();
    if literal.is_empty() {
        return None;
    }
    let probe = RepoPath::try_from(literal.join("/")).ok()?;
    let zone = ctx.policy_view().zone(&probe)?;
    (!zone.owners.is_empty()).then(|| zone.owners.iter().map(ToString::to_string).collect())
}

/// `prop_` and 16 random lowercase letters and digits.
fn new_proposal_id() -> Result<String, OpError> {
    const ALPHABET: &[u8; 32] = b"abcdefghijklmnopqrstuvwxyz234567";
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes)
        .map_err(|err| OpError::Unavailable(format!("no random numbers: {err}")))?;
    let mut id = String::from("prop_");
    for byte in bytes {
        id.push(char::from(ALPHABET[usize::from(byte) % ALPHABET.len()]));
    }
    Ok(id)
}

/// Lore for a pre-tool event: [`inject`] on every repository path the event names, reads
/// included. [`Normalized`] carries only writes and deletes (all the gate needs), so read paths
/// are resolved here, and only when the repository has lore at all.
#[must_use]
pub fn inject_for_event(
    ctx: &RepoContext,
    seat: &SeatId,
    event: &ToolEvent,
    norm: &Normalized,
    now: Timestamp,
) -> Vec<String> {
    if !ctx.layout.work_tree.join(LORE_DIR).is_dir() {
        return Vec::new();
    }
    let mut paths: Vec<String> = Vec::new();
    let add = |path: &str, paths: &mut Vec<String>| {
        if !path.is_empty() && paths.len() < MAX_PATHS && !paths.iter().any(|p| p == path) {
            paths.push(path.to_owned());
        }
    };
    for target in &norm.targets {
        if let PathLocation::InRepo(path) = &target.path.location {
            add(path.as_str(), &mut paths);
        }
    }
    let cwd = Path::new(&event.cwd);
    let mut reads: Vec<&str> = Vec::new();
    if event
        .tool
        .as_ref()
        .is_some_and(|t| t.kind == ToolKind::Read)
    {
        reads.extend(event.paths.iter().map(String::as_str));
    }
    if let Some(shell) = &norm.shell {
        for command in &shell.commands {
            reads.extend(
                command
                    .targets
                    .iter()
                    .filter(|t| t.op == TargetOp::Read && t.confidence == Confidence::Exact)
                    .map(|t| t.path.as_str()),
            );
        }
    }
    for raw in reads.into_iter().take(MAX_PATHS) {
        if let Some(path) = ctx.resolver.resolve(raw, cwd).repo_path() {
            add(path.as_str(), &mut paths);
        }
    }
    let refs: Vec<&str> = paths.iter().map(String::as_str).collect();
    inject(ctx, seat, &refs, now)
}

/// The staleness of each entry, from the cache for this `HEAD` when there is one. `None` for an
/// entry that could not be measured, and for all of them before the first commit.
fn measure_all(
    ctx: &RepoContext,
    work_tree: &Path,
    entries: &[&acs_lore::LoreEntry],
    threshold: acs_core::Percent,
    ci: bool,
) -> Vec<Option<Staleness>> {
    let head = acs_git::lore::head_commit(work_tree).ok().flatten();
    let Some(head) = head else {
        return vec![None; entries.len()];
    };
    match &ctx.repo_home {
        Some(repo) => {
            let mut cache = StaleCache::open(&repo.cache_dir(), &head, threshold);
            let out = entries
                .iter()
                .map(|entry| cache.get_or_measure(work_tree, entry, ci))
                .collect();
            cache.save();
            out
        }
        None => entries
            .iter()
            .map(|entry| acs_lore::stale::measure(work_tree, entry, &head, threshold, ci).ok())
            .collect(),
    }
}
