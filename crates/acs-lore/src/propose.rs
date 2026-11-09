//! Lore proposals: what an agent suggests, held until an owner reviews it.
//!
//! Agents cannot write `.acs/**`, so they propose through the MCP tool `acs_lore_propose`. A
//! proposal is one JSON file in the repository's `pending/lore/` directory under `ACS_HOME`,
//! outside the work tree. `acs lore review` turns an accepted one into a lore file committed on
//! a branch of its own (`acs/lore-<id>`), which goes through review like any other change.

use std::path::{Path, PathBuf};

use acs_core::{LoreFrontMatter, LoreId, Owner, Timestamp};
use acs_policy::glob::Glob;
use serde::{Deserialize, Serialize};

use crate::format::{LORE_DIR, LoreSet, render_file};

/// The longest proposed text, in characters (the default injection cap).
pub const MAX_BODY_CHARS: usize = 4_000;
/// The most scope globs a proposal may list.
pub const MAX_SCOPE: usize = 16;
/// The longest reason, in characters.
pub const MAX_WHY_CHARS: usize = 500;
/// The most proposals waiting at once; more are refused until some are reviewed.
pub const MAX_PENDING: usize = 200;

/// Who proposed it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Proposer {
    /// The pilot (git email) of the seat.
    pub pilot: String,
    /// The harness.
    pub harness: String,
    /// The `ses_` id of the session.
    pub session_id: String,
}

/// A proposed new lore entry, or a proposed replacement for an existing one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Proposal {
    /// `prop_` followed by lowercase letters and digits.
    pub proposal_id: String,
    /// The entry this replaces; `None` for a new entry.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub replaces: Option<LoreId>,
    /// The paths the entry applies to (globs).
    pub scope: Vec<String>,
    /// The entry's text (Markdown).
    pub body: String,
    /// Why the agent proposes it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub why: Option<String>,
    /// Who proposed it.
    pub proposed_by: Proposer,
    /// When.
    pub proposed_at: Timestamp,
    /// `HEAD` when it was proposed, if there was one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub head: Option<String>,
}

/// What an agent sends; checked by [`check`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Draft {
    /// The paths the entry applies to (globs). May be empty when `replaces` is set: the
    /// replaced entry's scope is kept.
    pub scope: Vec<String>,
    /// The entry's text.
    pub body: String,
    /// The id of the entry to replace.
    pub replaces: Option<String>,
    /// Why.
    pub why: Option<String>,
}

/// A draft that passed [`check`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Checked {
    /// Scope globs, trimmed, with `./` and leading `/` removed.
    pub scope: Vec<String>,
    /// Body, trimmed.
    pub body: String,
    /// The replaced entry.
    pub replaces: Option<LoreId>,
    /// Reason, trimmed; `None` when empty.
    pub why: Option<String>,
}

/// Checks a draft against the current lore. The error text is written for the agent.
///
/// # Errors
///
/// A message saying what to fix: empty or overlong text, bad globs, too many globs, or a
/// `replaces` that names no entry.
pub fn check(draft: &Draft, set: &LoreSet, case_insensitive: bool) -> Result<Checked, String> {
    let body = draft.body.trim();
    if body.is_empty() {
        return Err(
            "The lore text is empty. Pass `text`: the facts the next agent working \
                    here needs, in a few sentences."
                .to_owned(),
        );
    }
    let chars = body.chars().count();
    if chars > MAX_BODY_CHARS {
        return Err(format!(
            "The lore text is {chars} characters; keep it under {MAX_BODY_CHARS}. Lore is shown \
             to every agent working in its scope, so state only what is not obvious from the \
             code."
        ));
    }
    let replaces = match draft.replaces.as_deref().map(str::trim) {
        None | Some("") => None,
        Some(raw) => {
            let id: LoreId = raw.parse().map_err(|_| {
                format!("`{raw}` is not a lore id; lore ids look like `lore_7f3a`.")
            })?;
            if set.get(&id).is_none() {
                return Err(format!(
                    "There is no lore entry `{id}` to replace. Leave `replaces` out to propose a \
                     new entry."
                ));
            }
            Some(id)
        }
    };
    let mut scope: Vec<String> = draft
        .scope
        .iter()
        .map(|s| normalize_glob(s))
        .filter(|s| !s.is_empty())
        .collect();
    scope.dedup();
    if scope.is_empty() {
        match &replaces {
            Some(id) => {
                scope.clone_from(
                    &set.get(id)
                        .map(|e| e.front.scope.clone())
                        .unwrap_or_default(),
                );
            }
            None => {
                return Err(
                    "Say where the lore applies: pass `scope`, a list of path globs \
                            relative to the repository root, such as [\"payments/**\"]."
                        .to_owned(),
                );
            }
        }
    }
    if scope.len() > MAX_SCOPE {
        return Err(format!(
            "List at most {MAX_SCOPE} scope globs; use `**` to cover more."
        ));
    }
    for pattern in &scope {
        Glob::new(pattern, case_insensitive).map_err(|err| {
            format!(
                "Scope `{pattern}` is not a valid glob: {err}. {}. Use a path relative to the \
                 repository root, such as `payments/**`.",
                err.fix
            )
        })?;
    }
    let why = draft
        .why
        .as_deref()
        .map(str::trim)
        .filter(|w| !w.is_empty())
        .map(|w| w.chars().take(MAX_WHY_CHARS).collect::<String>());
    Ok(Checked {
        scope,
        body: body.to_owned(),
        replaces,
        why,
    })
}

/// Trims a glob and strips a leading `./` or `/` (agents write both), turning `\` into `/`.
fn normalize_glob(raw: &str) -> String {
    let text = raw.trim().replace('\\', "/");
    let text = text.strip_prefix("./").unwrap_or(&text);
    text.trim_start_matches('/').to_owned()
}

/// The pending proposals of one repository: a directory of JSON files.
#[derive(Debug, Clone)]
pub struct ProposalStore {
    dir: PathBuf,
}

/// Why a proposal file could not be used.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoreProblem {
    /// The file.
    pub file: PathBuf,
    /// What is wrong.
    pub message: String,
}

impl ProposalStore {
    /// The store in `dir` (normally `<repo home>/pending/lore`). Nothing is created until a
    /// proposal is saved.
    #[must_use]
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    /// The directory.
    #[must_use]
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Saves `proposal`, replacing a proposal with the same id.
    ///
    /// # Errors
    ///
    /// A message when the store is full or the file cannot be written.
    pub fn save(&self, proposal: &Proposal) -> Result<PathBuf, String> {
        if !valid_proposal_id(&proposal.proposal_id) {
            return Err(format!("invalid proposal id {:?}", proposal.proposal_id));
        }
        let (pending, _) = self.list();
        if pending.len() >= MAX_PENDING
            && !pending
                .iter()
                .any(|p| p.proposal_id == proposal.proposal_id)
        {
            return Err(format!(
                "{MAX_PENDING} lore proposals are already waiting for review; ask the user to \
                 review them with `acs lore review` before proposing more."
            ));
        }
        std::fs::create_dir_all(&self.dir)
            .map_err(|err| format!("cannot create {}: {err}", self.dir.display()))?;
        let path = self.path_of(&proposal.proposal_id);
        let bytes = serde_json::to_vec_pretty(proposal).map_err(|err| err.to_string())?;
        let tmp = path.with_extension(format!("json.{}.tmp", std::process::id()));
        std::fs::write(&tmp, bytes)
            .and_then(|()| std::fs::rename(&tmp, &path))
            .map_err(|err| {
                let _ = std::fs::remove_file(&tmp);
                format!("cannot write {}: {err}", path.display())
            })?;
        Ok(path)
    }

    /// Every pending proposal, oldest first, and the files that could not be read.
    #[must_use]
    pub fn list(&self) -> (Vec<Proposal>, Vec<StoreProblem>) {
        let mut proposals = Vec::new();
        let mut problems = Vec::new();
        let Ok(read) = std::fs::read_dir(&self.dir) else {
            return (proposals, problems);
        };
        for item in read.flatten() {
            let path = item.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            match std::fs::read(&path)
                .map_err(|err| err.to_string())
                .and_then(|bytes| {
                    serde_json::from_slice::<Proposal>(&bytes).map_err(|err| err.to_string())
                }) {
                Ok(proposal) => proposals.push(proposal),
                Err(message) => problems.push(StoreProblem {
                    file: path,
                    message,
                }),
            }
        }
        proposals
            .sort_by(|a, b| (a.proposed_at, &a.proposal_id).cmp(&(b.proposed_at, &b.proposal_id)));
        (proposals, problems)
    }

    /// The proposal `id`, if pending.
    #[must_use]
    pub fn get(&self, id: &str) -> Option<Proposal> {
        if !valid_proposal_id(id) {
            return None;
        }
        let bytes = std::fs::read(self.path_of(id)).ok()?;
        serde_json::from_slice(&bytes).ok()
    }

    /// Removes the proposal `id`. Returns whether it was pending.
    ///
    /// # Errors
    ///
    /// A message when the file exists but cannot be removed.
    pub fn remove(&self, id: &str) -> Result<bool, String> {
        if !valid_proposal_id(id) {
            return Ok(false);
        }
        let path = self.path_of(id);
        match std::fs::remove_file(&path) {
            Ok(()) => Ok(true),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(err) => Err(format!("cannot remove {}: {err}", path.display())),
        }
    }

    fn path_of(&self, id: &str) -> PathBuf {
        self.dir.join(format!("{id}.json"))
    }
}

/// Whether `id` is `prop_` followed by 1 to 64 lowercase letters, digits, `_` or `-`.
#[must_use]
pub fn valid_proposal_id(id: &str) -> bool {
    id.strip_prefix("prop_").is_some_and(|body| {
        (1..=64).contains(&body.len())
            && body
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-')
    })
}

/// Where a new entry's file goes: under `.acs/policy/lore/`, mirroring the literal leading
/// directories its first scope glob names (`payments/api/**` goes to
/// `.acs/policy/lore/payments/api/`), named after the id without its prefix.
#[must_use]
pub fn file_for(scope: &[String], id: &LoreId) -> String {
    let dirs: Vec<&str> = scope
        .first()
        .map(|first| {
            let segments: Vec<&str> = first.split('/').collect();
            let literal = segments
                .iter()
                .take_while(|seg| !seg.contains(['*', '?', '[', '{']))
                .count();
            // The last segment of a fully literal glob is a file, not a directory.
            let dirs = if literal == segments.len() {
                literal.saturating_sub(1)
            } else {
                literal
            };
            segments[..dirs].to_vec()
        })
        .unwrap_or_default();
    let name = id
        .as_str()
        .strip_prefix(LoreId::PREFIX)
        .unwrap_or(id.as_str());
    let mut path = LORE_DIR.to_owned();
    for dir in dirs {
        path.push('/');
        path.push_str(dir);
    }
    format!("{path}/{name}.md")
}

/// The lore file an accepted proposal becomes: its path (the replaced entry's file, or
/// [`file_for`]) and its text.
///
/// # Errors
///
/// Only if the front matter cannot be rendered.
pub fn accepted_file(
    proposal: &Proposal,
    id: LoreId,
    owner: Owner,
    written_against: String,
    set: &LoreSet,
) -> Result<(String, String), String> {
    let path = proposal
        .replaces
        .as_ref()
        .and_then(|old| set.get(old))
        .map_or_else(
            || file_for(&proposal.scope, &id),
            |entry| entry.file.clone(),
        );
    let front = LoreFrontMatter {
        id,
        scope: proposal.scope.clone(),
        owner,
        written_against,
    };
    Ok((path, render_file(&front, &proposal.body)?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::format::LoreEntry;

    fn set() -> LoreSet {
        let text = "+++\nid = \"lore_7f3a\"\nscope = [\"payments/**\"]\nowner = \"sara@example.com\"\n\
                    written_against = \"a1b2c3d\"\n+++\nCents.\n";
        LoreSet::from_parsed(vec![LoreEntry::parse(
            ".acs/policy/lore/payments/money.md",
            text,
            false,
        )])
    }

    fn draft(scope: &[&str], body: &str) -> Draft {
        Draft {
            scope: scope.iter().map(|s| (*s).to_owned()).collect(),
            body: body.to_owned(),
            ..Draft::default()
        }
    }

    #[test]
    fn check_accepts_and_normalizes() {
        let checked = check(
            &draft(&["./src/**", "/lib/**", " "], "  Use X.  "),
            &set(),
            false,
        )
        .unwrap();
        assert_eq!(checked.scope, ["src/**", "lib/**"]);
        assert_eq!(checked.body, "Use X.");
    }

    #[test]
    fn check_explains_what_to_fix() {
        let set = set();
        let cases = [
            (draft(&["a/**"], " "), "empty"),
            (draft(&[], "Body"), "Say where the lore applies"),
            (draft(&["a/../b"], "Body"), "not a valid glob"),
            (
                draft(&["a/**"], &"x".repeat(MAX_BODY_CHARS + 1)),
                "characters",
            ),
            (
                Draft {
                    replaces: Some("lore_nope".into()),
                    ..draft(&["a/**"], "Body")
                },
                "no lore entry",
            ),
            (
                Draft {
                    replaces: Some("seat_x".into()),
                    ..draft(&["a/**"], "Body")
                },
                "not a lore id",
            ),
        ];
        for (d, expected) in cases {
            let err = check(&d, &set, false).unwrap_err();
            assert!(err.contains(expected), "{d:?}: {err}");
        }
    }

    #[test]
    fn replacing_keeps_the_scope_when_none_is_given() {
        let d = Draft {
            replaces: Some("lore_7f3a".into()),
            ..draft(&[], "Dollars now.")
        };
        let checked = check(&d, &set(), false).unwrap();
        assert_eq!(checked.scope, ["payments/**"]);
        assert_eq!(checked.replaces.unwrap().as_str(), "lore_7f3a");
    }

    #[test]
    fn files_mirror_the_scope() {
        let id: LoreId = "lore_ab12".parse().unwrap();
        let file = |s: &str| file_for(&[s.to_owned()], &id);
        assert_eq!(
            file("payments/api/**"),
            ".acs/policy/lore/payments/api/ab12.md"
        );
        assert_eq!(file("src/lib.rs"), ".acs/policy/lore/src/ab12.md");
        assert_eq!(file("**/*.sql"), ".acs/policy/lore/ab12.md");
        assert_eq!(file_for(&[], &id), ".acs/policy/lore/ab12.md");
    }

    fn proposal(id: &str, at: i64) -> Proposal {
        Proposal {
            proposal_id: id.to_owned(),
            replaces: None,
            scope: vec!["src/**".into()],
            body: "Body.".into(),
            why: None,
            proposed_by: Proposer {
                pilot: "omar@example.com".into(),
                harness: "claude-code".into(),
                session_id: "ses_1".into(),
            },
            proposed_at: Timestamp::from_second(at).unwrap(),
            head: None,
        }
    }

    #[test]
    fn store_round_trips_lists_in_order_and_removes() {
        let dir = tempfile::tempdir().unwrap();
        let store = ProposalStore::new(dir.path().join("pending").join("lore"));
        assert!(store.list().0.is_empty());
        store.save(&proposal("prop_b", 20)).unwrap();
        store.save(&proposal("prop_a", 10)).unwrap();
        std::fs::write(store.dir().join("prop_bad.json"), "{").unwrap();
        let (list, problems) = store.list();
        let ids: Vec<_> = list.iter().map(|p| p.proposal_id.as_str()).collect();
        assert_eq!(ids, ["prop_a", "prop_b"]);
        assert_eq!(problems.len(), 1);
        assert_eq!(store.get("prop_a"), Some(proposal("prop_a", 10)));
        assert!(store.get("../x").is_none());
        assert!(store.remove("prop_a").unwrap());
        assert!(!store.remove("prop_a").unwrap());
        assert!(store.save(&proposal("../evil", 1)).is_err());
    }

    #[test]
    fn accepted_file_replaces_in_place_or_creates() {
        let set = set();
        let owner: Owner = "sara@example.com".to_owned().try_into().unwrap();
        let mut p = proposal("prop_a", 1);
        let (path, text) = accepted_file(
            &p,
            "lore_ab12".parse().unwrap(),
            owner.clone(),
            "0123abc".into(),
            &set,
        )
        .unwrap();
        assert_eq!(path, ".acs/policy/lore/src/ab12.md");
        let entry = LoreEntry::parse(&path, &text, false).unwrap();
        assert_eq!(entry.body, "Body.");
        assert_eq!(entry.front.written_against, "0123abc");

        p.replaces = Some("lore_7f3a".parse().unwrap());
        let (path, _) =
            accepted_file(&p, "lore_7f3a".parse().unwrap(), owner, "x".into(), &set).unwrap();
        assert_eq!(path, ".acs/policy/lore/payments/money.md");
    }
}
