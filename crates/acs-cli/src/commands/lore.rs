//! `acs lore`: read the repository's lore and review what agents propose (WP6.1, WP6.2,
//! decision 0029).
//!
//! `list`, `show` and `stale` only read, so agents may run them too. `review` changes the
//! repository (it commits accepted entries on a branch `acs/lore-<id>`), so the hook denies it to
//! agents (decision 0026): proposals reach a person, never the policy directly.

use std::io::{BufRead, IsTerminal, Write};
use std::process::ExitCode;

use acs_core::{LoreId, Owner, Timestamp};
use acs_lore::propose::{Proposal, ProposalStore, accepted_file};
use acs_lore::resolve::{Served, render};
use acs_lore::{LoreEntry, LoreSet, StaleCache, Staleness, resolve};
use clap::{Args, Subcommand};
use serde_json::{Value, json};

use super::support::{CliError, OutputArgs, ellipsize_end, finish, open_repo, print_json, wrap};
use crate::runtime::{OpError, RepoContext, lore as rt};

/// Arguments of `acs lore`.
#[derive(Debug, Args)]
#[command(
    after_help = "Lore is short facts about parts of the repository, kept in \
    .acs/policy/lore/ and shown to an agent the first time it works in an entry's scope in a \
    session. Agents propose lore with the acs_lore_propose tool; you review it here.\n\n\
    Examples:\n  \
    acs lore show src/payments/refund.ts   what an agent working there is told\n  \
    acs lore stale                         entries whose code changed since they were written\n  \
    acs lore review                        go through proposals one by one"
)]
pub struct LoreArgs {
    /// What to do.
    #[command(subcommand)]
    pub command: LoreCommand,
}

/// The subcommands of `acs lore`.
#[derive(Debug, Subcommand)]
pub enum LoreCommand {
    /// List the lore entries, broken lore files, and proposals waiting for review.
    List(ListArgs),
    /// Show the lore that applies to files, as an agent working on them is shown it.
    Show(ShowArgs),
    /// List the entries whose scope changed a lot since they were written.
    Stale(StaleArgs),
    /// Accept, edit or reject the lore agents proposed.
    Review(ReviewArgs),
}

/// Arguments of `acs lore list`.
#[derive(Debug, Args)]
pub struct ListArgs {
    /// Output format.
    #[command(flatten)]
    pub output: OutputArgs,
}

/// Arguments of `acs lore show`.
#[derive(Debug, Args)]
pub struct ShowArgs {
    /// Files or directories, relative to the current directory.
    #[arg(required = true, value_name = "PATH")]
    pub paths: Vec<String>,
    /// Output format.
    #[command(flatten)]
    pub output: OutputArgs,
}

/// Arguments of `acs lore stale`.
#[derive(Debug, Args)]
pub struct StaleArgs {
    /// List every entry with how much of its scope changed, not only the stale ones.
    #[arg(long)]
    pub all: bool,
    /// Output format.
    #[command(flatten)]
    pub output: OutputArgs,
}

/// Arguments of `acs lore review`.
#[derive(Debug, Args)]
#[command(
    after_help = "With no options it goes through the proposals one by one and asks \
    what to do with each (in a terminal). An accepted proposal becomes a lore file committed on \
    the branch acs/lore-<id>, on top of HEAD; your work tree, index and branch are not touched. \
    Merge that branch, or push it and open a pull request, to put the entry in force.\n\n\
    Examples:\n  \
    acs lore review --list\n  \
    acs lore review --accept prop_k2m4...\n  \
    acs lore review --accept prop_k2m4... --body-file edited.md\n  \
    acs lore review --reject prop_k2m4..."
)]
pub struct ReviewArgs {
    /// List the proposals without reviewing them.
    #[arg(long, conflicts_with_all = ["accept", "reject"])]
    pub list: bool,
    /// Accept a proposal.
    #[arg(long, value_name = "ID", conflicts_with = "reject")]
    pub accept: Option<String>,
    /// Reject a proposal (it is deleted).
    #[arg(long, value_name = "ID")]
    pub reject: Option<String>,
    /// With --accept: use this file's text instead of the proposed text.
    #[arg(long, value_name = "FILE", requires = "accept")]
    pub body_file: Option<String>,
    /// With --accept: the owner written into the entry (an email or team:<name>). Defaults to
    /// the replaced entry's owner, else the owner of the zone it covers, else you.
    #[arg(long, value_name = "OWNER")]
    pub owner: Option<String>,
    /// Output format.
    #[command(flatten)]
    pub output: OutputArgs,
}

/// Runs `acs lore`.
#[must_use]
pub fn run(args: &LoreArgs) -> ExitCode {
    match &args.command {
        LoreCommand::List(a) => finish(a.output.json, list(a.output.json)),
        LoreCommand::Show(a) => finish(a.output.json, show(a)),
        LoreCommand::Stale(a) => finish(a.output.json, stale(a)),
        LoreCommand::Review(a) => finish(a.output.json, review(a)),
    }
}

fn load(ctx: &RepoContext) -> LoreSet {
    LoreSet::load(&ctx.layout.work_tree, ctx.resolver.case_insensitive())
}

fn entry_json(entry: &LoreEntry) -> Value {
    json!({
        "id": entry.id(),
        "scope": entry.front.scope,
        "owner": entry.front.owner.to_string(),
        "written_against": entry.front.written_against,
        "file": entry.file,
    })
}

// ---------------------------------------------------------------------------------------------
// list

fn list(json_out: bool) -> Result<ExitCode, CliError> {
    let (ctx, _) = open_repo()?;
    let set = load(&ctx);
    let (pending, _) = rt::proposals(&ctx).map_or_else(|_| (Vec::new(), Vec::new()), |s| s.list());
    if json_out {
        print_json(&json!({
            "ok": true,
            "entries": set.entries().iter().map(entry_json).collect::<Vec<_>>(),
            "problems": set.problems().iter().map(|p| json!({"file": p.file, "message": p.message})).collect::<Vec<_>>(),
            "pending_proposals": pending.len(),
        }));
        return Ok(ExitCode::SUCCESS);
    }
    let mut out = String::new();
    if set.entries().is_empty() {
        out.push_str("No lore entries (.acs/policy/lore/ has none).\n");
    } else {
        let n = set.entries().len();
        out.push_str(&format!(
            "{n} lore entr{}:\n",
            if n == 1 { "y" } else { "ies" }
        ));
        for entry in set.entries() {
            out.push_str(&format!(
                "  {}  {}  ({})\n      {}\n",
                entry.id(),
                ellipsize_end(&entry.front.scope.join(", "), 40),
                ellipsize_end(&entry.front.owner.to_string(), 30),
                ellipsize_end(&entry.file, 72),
            ));
        }
    }
    if !set.problems().is_empty() {
        out.push_str("Left out (fix these files):\n");
        for problem in set.problems() {
            out.push_str(&wrap(&problem.to_string(), "  ", "    "));
        }
    }
    if !pending.is_empty() {
        out.push_str(&format!(
            "{} proposal{} wait{} for review: acs lore review\n",
            pending.len(),
            if pending.len() == 1 { "" } else { "s" },
            if pending.len() == 1 { "s" } else { "" },
        ));
    }
    print!("{out}");
    Ok(ExitCode::SUCCESS)
}

// ---------------------------------------------------------------------------------------------
// show

fn show(args: &ShowArgs) -> Result<ExitCode, CliError> {
    let (ctx, cwd) = open_repo()?;
    let mut paths = Vec::new();
    for raw in &args.paths {
        let resolved = ctx.resolver.resolve(raw, &cwd);
        match resolved.repo_path() {
            Some(path) => paths.push(path.as_str().to_owned()),
            None => {
                return Err(CliError::new(
                    "outside",
                    format!("`{raw}` is not inside this repository."),
                ));
            }
        }
    }
    let set = load(&ctx);
    let refs: Vec<&str> = paths.iter().map(String::as_str).collect();
    let matches = resolve(&set, &refs);
    let mut cache = stale_cache(&ctx);
    let staleness: Vec<Option<Staleness>> = matches
        .iter()
        .map(|m| measure(&ctx, cache.as_mut(), m.entry))
        .collect();
    if let Some(cache) = cache.as_mut() {
        cache.save();
    }
    if args.output.json {
        let entries: Vec<Value> = matches
            .iter()
            .zip(&staleness)
            .map(|(m, s)| {
                let mut value = entry_json(m.entry);
                value["body"] = json!(m.entry.body);
                value["staleness"] = json!(s);
                value
            })
            .collect();
        print_json(&json!({"ok": true, "paths": paths, "entries": entries}));
        return Ok(ExitCode::SUCCESS);
    }
    if matches.is_empty() {
        println!("No lore applies to {}.", ticked(&paths));
        return Ok(ExitCode::SUCCESS);
    }
    let served: Vec<Served<'_>> = matches
        .iter()
        .zip(&staleness)
        .map(|(m, s)| Served {
            entry: m.entry,
            staleness: s.as_ref(),
        })
        .collect();
    let cap = usize::try_from(ctx.policy.policy.files().config.lore.max_inject_chars.get())
        .unwrap_or(usize::MAX);
    // Everything, uncapped: a person asking wants it all. The cap is shown for reference.
    let all = render(&served, usize::MAX);
    let capped = render(&served, cap);
    println!("{}", all.blocks.join("\n\n"));
    if !capped.deferred.is_empty() || !capped.cut.is_empty() {
        println!(
            "\n(An agent gets at most {cap} characters per tool call; the rest comes with its \
             next calls in these scopes.)"
        );
    }
    Ok(ExitCode::SUCCESS)
}

fn ticked(items: &[String]) -> String {
    items
        .iter()
        .map(|p| format!("`{p}`"))
        .collect::<Vec<_>>()
        .join(", ")
}

// ---------------------------------------------------------------------------------------------
// stale

fn stale_cache(ctx: &RepoContext) -> Option<StaleCache> {
    let head = acs_git::lore::head_commit(&ctx.layout.work_tree).ok()??;
    let repo = ctx.repo_home.as_ref()?;
    let threshold = ctx.policy.policy.files().config.lore.stale_percent;
    Some(StaleCache::open(&repo.cache_dir(), &head, threshold))
}

fn measure(
    ctx: &RepoContext,
    cache: Option<&mut StaleCache>,
    entry: &LoreEntry,
) -> Option<Staleness> {
    let ci = ctx.resolver.case_insensitive();
    match cache {
        Some(cache) => cache.get_or_measure(&ctx.layout.work_tree, entry, ci),
        None => {
            let head = acs_git::lore::head_commit(&ctx.layout.work_tree).ok()??;
            let threshold = ctx.policy.policy.files().config.lore.stale_percent;
            acs_lore::stale::measure(&ctx.layout.work_tree, entry, &head, threshold, ci).ok()
        }
    }
}

fn stale(args: &StaleArgs) -> Result<ExitCode, CliError> {
    let (ctx, _) = open_repo()?;
    if acs_git::lore::head_commit(&ctx.layout.work_tree)
        .ok()
        .flatten()
        .is_none()
    {
        return Err(CliError::new(
            "no_commits",
            "The repository has no commits yet, so there is nothing to measure against.",
        ));
    }
    let set = load(&ctx);
    let mut cache = stale_cache(&ctx);
    let rows: Vec<(&LoreEntry, Option<Staleness>)> = set
        .entries()
        .iter()
        .map(|entry| (entry, measure(&ctx, cache.as_mut(), entry)))
        .collect();
    if let Some(cache) = cache.as_mut() {
        cache.save();
    }
    let threshold = ctx.policy.policy.files().config.lore.stale_percent.get();
    let shown: Vec<&(&LoreEntry, Option<Staleness>)> = rows
        .iter()
        .filter(|(_, s)| args.all || s.as_ref().is_none_or(Staleness::is_stale))
        .collect();
    if args.output.json {
        let entries: Vec<Value> = shown
            .iter()
            .map(|(entry, s)| {
                let mut value = entry_json(entry);
                value["staleness"] = json!(s);
                value
            })
            .collect();
        print_json(&json!({"ok": true, "threshold_percent": threshold, "entries": entries}));
        return Ok(ExitCode::SUCCESS);
    }
    if shown.is_empty() {
        println!(
            "No stale lore: in every entry's scope, less than {threshold}% of the lines changed \
             since it was written."
        );
        return Ok(ExitCode::SUCCESS);
    }
    for (entry, s) in &shown {
        let state = s
            .as_ref()
            .map_or_else(|| "could not be measured".to_owned(), Staleness::summary);
        println!(
            "{}  {}  {}\n      {}",
            entry.id(),
            ellipsize_end(&entry.front.scope.join(", "), 30),
            state,
            ellipsize_end(&entry.file, 72)
        );
    }
    if !args.all {
        println!(
            "\nAgents are shown these with a request to verify them. Update an entry and its \
             written_against, or run `acs lore review` for corrections agents proposed."
        );
    }
    Ok(ExitCode::SUCCESS)
}

// ---------------------------------------------------------------------------------------------
// review

fn review(args: &ReviewArgs) -> Result<ExitCode, CliError> {
    let (ctx, _) = open_repo()?;
    let store = rt::proposals(&ctx).map_err(CliError::from)?;
    let json_out = args.output.json;
    if let Some(id) = &args.reject {
        return reject(&store, id, json_out);
    }
    if let Some(id) = &args.accept {
        let proposal = store.get(id).ok_or_else(|| not_pending(id))?;
        let body = match &args.body_file {
            Some(file) => Some(
                std::fs::read_to_string(file)
                    .map_err(|err| CliError::new("io", format!("cannot read {file}: {err}")))?,
            ),
            None => None,
        };
        let accepted = accept(&ctx, &store, &proposal, body, args.owner.as_deref())?;
        print_accepted(&accepted, json_out);
        return Ok(ExitCode::SUCCESS);
    }
    let (pending, problems) = store.list();
    if args.list || json_out {
        if json_out {
            print_json(&json!({
                "ok": true,
                "proposals": pending,
                "problems": problems.iter().map(|p| json!({"file": p.file, "message": p.message})).collect::<Vec<_>>(),
            }));
        } else if pending.is_empty() {
            println!("No lore proposals wait for review.");
        } else {
            let set = load(&ctx);
            for proposal in &pending {
                print!("{}", describe(&ctx, &set, proposal, false));
            }
        }
        return Ok(ExitCode::SUCCESS);
    }
    interactive(&ctx, &store, &pending)
}

fn not_pending(id: &str) -> CliError {
    CliError::new(
        "not_found",
        format!("No proposal `{id}` waits for review; `acs lore review --list` shows them."),
    )
}

fn reject(store: &ProposalStore, id: &str, json_out: bool) -> Result<ExitCode, CliError> {
    if !store.remove(id).map_err(|err| CliError::new("io", err))? {
        return Err(not_pending(id));
    }
    if json_out {
        print_json(&json!({"ok": true, "rejected": id}));
    } else {
        println!("Rejected {id}; it is deleted.");
    }
    Ok(ExitCode::SUCCESS)
}

/// A proposal as text: who, when, where, and the body (cut unless `full`).
fn describe(ctx: &RepoContext, set: &LoreSet, proposal: &Proposal, full: bool) -> String {
    let by = &proposal.proposed_by;
    let mut text = format!(
        "{}  from {} ({}), {}\n",
        proposal.proposal_id,
        by.pilot,
        by.harness,
        Timestamp::from_second(proposal.proposed_at.as_second()).unwrap_or(proposal.proposed_at)
    );
    let scope = proposal.scope.join(", ");
    match &proposal.replaces {
        Some(id) => text.push_str(&wrap(
            &format!("Replaces {id}, scope {scope}"),
            "  ",
            "    ",
        )),
        None => text.push_str(&wrap(&format!("New entry, scope {scope}"), "  ", "    ")),
    }
    if let Some(reviewer) = rt::reviewer_of(ctx, set, proposal) {
        text.push_str(&wrap(&format!("Owner: {reviewer}"), "  ", "    "));
    }
    if let Some(why) = &proposal.why {
        text.push_str(&wrap(&format!("Why: {why}"), "  ", "    "));
    }
    let body = if full {
        proposal.body.clone()
    } else {
        ellipsize_end(&proposal.body.replace('\n', " "), 150)
    };
    for line in body.lines() {
        text.push_str(&wrap(line, "  | ", "  | "));
    }
    text.push('\n');
    text
}

/// What accepting a proposal did.
struct Accepted {
    proposal_id: String,
    lore_id: LoreId,
    file: String,
    branch: String,
    commit: String,
    signed: bool,
    owner: String,
    owner_note: Option<String>,
}

fn accept(
    ctx: &RepoContext,
    store: &ProposalStore,
    proposal: &Proposal,
    body: Option<String>,
    owner: Option<&str>,
) -> Result<Accepted, CliError> {
    let work_tree = &ctx.layout.work_tree;
    let set = load(ctx);
    let mut proposal = proposal.clone();
    if let Some(body) = body {
        let body = body.trim().to_owned();
        if body.is_empty() {
            return Err(CliError::new(
                "invalid",
                "The edited text is empty; nothing was accepted.",
            ));
        }
        proposal.body = body;
    }
    if let Some(old) = &proposal.replaces
        && set.get(old).is_none()
    {
        return Err(CliError::new(
            "invalid",
            format!(
                "{} replaces `{old}`, which no longer exists. Reject it, or accept it as a new \
                 entry by editing the proposal file in {}.",
                proposal.proposal_id,
                store.dir().display()
            ),
        ));
    }
    let me = acs_git::repo::pilot_email_or_fallback(work_tree);
    let owner = pick_owner(ctx, &set, &proposal, owner, &me)?;
    let owner_note = owner_note(&owner, &me, ctx);
    let lore_id = match &proposal.replaces {
        Some(id) => id.clone(),
        None => new_lore_id(&set)?,
    };
    let head = acs_git::lore::head_commit(work_tree)
        .ok()
        .flatten()
        .ok_or_else(|| {
            CliError::new(
                "no_commits",
                "The repository has no commits yet; make a first commit before accepting lore.",
            )
        })?;
    let written_against = acs_git::lore::short_commit(work_tree, &head).unwrap_or(head);
    let (file, text) = accepted_file(
        &proposal,
        lore_id.clone(),
        owner.clone(),
        written_against,
        &set,
    )
    .map_err(|err| CliError::new("invalid", err))?;
    let suffix = lore_id
        .as_str()
        .strip_prefix(LoreId::PREFIX)
        .unwrap_or(lore_id.as_str());
    let branch = format!("acs/lore-{suffix}");
    let verb = if proposal.replaces.is_some() {
        "Update"
    } else {
        "Add"
    };
    let mut message = format!(
        "{verb} lore {lore_id} for {}\n\nProposed by {} ({}, session {}) as {}; accepted with \
         `acs lore review`.",
        proposal.scope.join(", "),
        proposal.proposed_by.pilot,
        proposal.proposed_by.harness,
        proposal.proposed_by.session_id,
        proposal.proposal_id,
    );
    if let Some(why) = &proposal.why {
        message.push_str(&format!("\n\nWhy: {why}"));
    }
    let made = acs_git::lore::commit_file_on_branch(
        work_tree,
        &branch,
        "HEAD",
        &file,
        text.as_bytes(),
        &message,
    )
    .map_err(|err| CliError::new("git", format!("could not commit the entry: {err}")))?;
    store
        .remove(&proposal.proposal_id)
        .map_err(|err| CliError::new("io", err))?;
    Ok(Accepted {
        proposal_id: proposal.proposal_id.clone(),
        lore_id,
        file,
        branch,
        commit: made.commit,
        signed: made.signed,
        owner: owner.to_string(),
        owner_note,
    })
}

/// The owner written into an accepted entry.
fn pick_owner(
    ctx: &RepoContext,
    set: &LoreSet,
    proposal: &Proposal,
    given: Option<&str>,
    me: &str,
) -> Result<Owner, CliError> {
    let parse = |raw: &str| {
        Owner::try_from(raw.to_owned()).map_err(|err| {
            CliError::new(
                "invalid",
                format!("{err}. Pass --owner name@domain or --owner team:<name>."),
            )
        })
    };
    if let Some(raw) = given {
        return parse(raw);
    }
    if let Some(entry) = proposal.replaces.as_ref().and_then(|id| set.get(id)) {
        return Ok(entry.front.owner.clone());
    }
    if let Some(first) = proposal
        .scope
        .first()
        .and_then(|glob| rt::zone_owners(ctx, glob))
        .and_then(|owners| owners.into_iter().next())
    {
        return parse(&first);
    }
    parse(me)
}

/// A note when the person accepting is not the entry's owner (they may still accept: the
/// branch goes through review, where CODEOWNERS can require the owner).
fn owner_note(owner: &Owner, me: &str, ctx: &RepoContext) -> Option<String> {
    let mine = match owner {
        Owner::Pilot(email) => email.eq_ignore_ascii_case(me),
        Owner::Team(name) => ctx
            .policy
            .policy
            .files()
            .territory
            .teams
            .get(name)
            .is_some_and(|members| members.iter().any(|m| m.eq_ignore_ascii_case(me))),
    };
    (!mine)
        .then(|| format!("The entry's owner is {owner}, not you; ask them to review the branch."))
}

/// A new `lore_` id: 8 random lowercase letters and digits, not used by any entry.
fn new_lore_id(set: &LoreSet) -> Result<LoreId, CliError> {
    const ALPHABET: &[u8; 32] = b"abcdefghijklmnopqrstuvwxyz234567";
    for _ in 0..16 {
        let mut bytes = [0u8; 8];
        getrandom::fill(&mut bytes)
            .map_err(|err| CliError::new("random", format!("no random numbers: {err}")))?;
        let body: String = bytes
            .iter()
            .map(|b| char::from(ALPHABET[usize::from(*b) % ALPHABET.len()]))
            .collect();
        if let Ok(id) = format!("lore_{body}").parse::<LoreId>()
            && set.get(&id).is_none()
        {
            return Ok(id);
        }
    }
    Err(CliError::new("random", "could not pick an unused lore id"))
}

fn print_accepted(accepted: &Accepted, json_out: bool) {
    if json_out {
        print_json(&json!({
            "ok": true,
            "accepted": accepted.proposal_id,
            "lore_id": accepted.lore_id,
            "file": accepted.file,
            "branch": accepted.branch,
            "commit": accepted.commit,
            "signed": accepted.signed,
            "owner": accepted.owner,
            "note": accepted.owner_note,
        }));
        return;
    }
    let short: String = accepted.commit.chars().take(12).collect();
    let mut text = wrap(
        &format!(
            "Accepted {} as {} ({}), committed on branch {} ({short}{}). Your work tree and \
             current branch are unchanged.",
            accepted.proposal_id,
            accepted.lore_id,
            accepted.file,
            accepted.branch,
            if accepted.signed { ", signed" } else { "" },
        ),
        "",
        "",
    );
    text.push_str(&wrap(
        &format!(
            "To put it in force, merge it (git merge {b}) or push it and open a pull request \
             (git push -u origin {b}).",
            b = accepted.branch
        ),
        "",
        "",
    ));
    if let Some(note) = &accepted.owner_note {
        text.push_str(&wrap(note, "", ""));
    }
    print!("{text}");
}

fn interactive(
    ctx: &RepoContext,
    store: &ProposalStore,
    pending: &[Proposal],
) -> Result<ExitCode, CliError> {
    if pending.is_empty() {
        println!("No lore proposals wait for review.");
        return Ok(ExitCode::SUCCESS);
    }
    if !(std::io::stdin().is_terminal() && std::io::stderr().is_terminal()) {
        return Err(CliError::new(
            "not_interactive",
            "acs lore review asks what to do with each proposal, so it needs a terminal. Use \
             --list, --accept <id> or --reject <id> instead.",
        ));
    }
    let mut stdin = std::io::stdin().lock();
    let mut stderr = std::io::stderr();
    let total = pending.len();
    for (n, proposal) in pending.iter().enumerate() {
        let set = load(ctx);
        let _ = write!(
            stderr,
            "\n[{}/{total}] {}",
            n + 1,
            describe(ctx, &set, proposal, true)
        );
        loop {
            let _ = write!(stderr, "Accept, edit, reject, skip or quit? [a/e/r/s/q] ");
            let _ = stderr.flush();
            let mut line = String::new();
            if stdin.read_line(&mut line).unwrap_or(0) == 0 {
                return Ok(ExitCode::SUCCESS);
            }
            match line.trim().to_ascii_lowercase().as_str() {
                "a" | "accept" => {
                    let accepted = accept(ctx, store, proposal, None, None)?;
                    print_accepted(&accepted, false);
                    break;
                }
                "e" | "edit" => {
                    if let Some(body) = edit(ctx, proposal, &mut stdin, &mut stderr)? {
                        let accepted = accept(ctx, store, proposal, Some(body), None)?;
                        print_accepted(&accepted, false);
                        break;
                    }
                }
                "r" | "reject" => {
                    reject(store, &proposal.proposal_id, false)?;
                    break;
                }
                "s" | "skip" | "" => break,
                "q" | "quit" => return Ok(ExitCode::SUCCESS),
                _ => {}
            }
        }
    }
    Ok(ExitCode::SUCCESS)
}

/// Writes the proposed text to a file for the person to edit in their own editor, waits for
/// Enter, and returns the edited text (`None` when they give up).
fn edit(
    ctx: &RepoContext,
    proposal: &Proposal,
    stdin: &mut impl BufRead,
    stderr: &mut impl Write,
) -> Result<Option<String>, CliError> {
    let dir = ctx
        .repo_home
        .as_ref()
        .map(|repo| repo.pending_dir().join("lore"))
        .ok_or_else(|| CliError::from(OpError::Unavailable("no state directory".to_owned())))?;
    let path = dir.join(format!("{}.edit.md", proposal.proposal_id));
    std::fs::write(&path, format!("{}\n", proposal.body))
        .map_err(|err| CliError::new("io", format!("cannot write {}: {err}", path.display())))?;
    let _ = write!(
        stderr,
        "{}",
        wrap(
            &format!(
                "Edit the text in {} with any editor, save it, then press Enter here (or type \
                 c and Enter to cancel).",
                path.display()
            ),
            "",
            ""
        )
    );
    let _ = stderr.flush();
    let mut line = String::new();
    let cancelled = stdin.read_line(&mut line).unwrap_or(0) == 0 || line.trim() == "c";
    let text = std::fs::read_to_string(&path);
    let _ = std::fs::remove_file(&path);
    if cancelled {
        return Ok(None);
    }
    let text =
        text.map_err(|err| CliError::new("io", format!("cannot read {}: {err}", path.display())))?;
    Ok(Some(text))
}
