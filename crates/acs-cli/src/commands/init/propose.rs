//! WP5.3: the territory `acs init` proposes for a repository (decision 0025).
//!
//! Two deterministic sources, no LLM and no CODEOWNERS:
//!
//! 1. **Folder names.** Top-level and second-level folders named like documentation, examples or
//!    scratch space (`docs/`, `examples/`, `scratch/`, ...) are proposed as `open`.
//! 2. **Git history.** For each top-level folder, and each second-level folder under a top-level
//!    one that has no clear owner, the commits of the last months that touched it are counted per
//!    author. A folder with enough commits where one author made a clear majority is proposed as
//!    `owned` by that author. Owned zones need at least two authors in the window: in a one-person
//!    repository every folder would be owned by the same person, which says nothing.
//!
//! Everything else stays `shared`, the default, so it needs no zone. Sealed paths are the built-in
//! defaults and are not proposed again. The person reviews the result (the setup skill shows it to
//! them) and edits `territory.toml` freely.
//!
//! [`propose`] is pure; [`history`] and [`tree_dirs`] read git.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fmt::Write as _;
use std::path::Path;
use std::process::{Command, Stdio};

use acs_core::{Owner, ZoneKind};
use serde::Serialize;

/// Folder names proposed as `open`, compared without case.
pub const OPEN_FOLDER_NAMES: &[&str] = &[
    "doc",
    "docs",
    "documentation",
    "example",
    "examples",
    "samples",
    "sandbox",
    "playground",
    "scratch",
    "tmp",
];

/// The most commits read from history; older ones in the window are ignored.
pub const MAX_COMMITS: usize = 20_000;

/// When a folder counts as owned.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Thresholds {
    /// The share of a folder's commits one author must have made, in percent.
    pub min_share_percent: u8,
    /// The fewest commits a folder needs in the window before anyone can own it.
    pub min_commits: u32,
    /// How many months of history are read.
    pub months: u32,
}

impl Default for Thresholds {
    fn default() -> Self {
        Self {
            min_share_percent: 75,
            min_commits: 10,
            months: 12,
        }
    }
}

/// One commit as the proposal sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Commit {
    /// The author's email (after `.mailmap`), lowercased.
    pub author: String,
    /// The paths it touched, relative to the work tree, with `/` separators.
    pub files: Vec<String>,
}

/// One proposed zone.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ProposedZone {
    /// The folder, relative to the work tree, without a trailing slash.
    pub folder: String,
    /// The zone kind: `open` or `owned`.
    pub kind: ZoneKind,
    /// The owner, for an `owned` zone.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub owners: Vec<String>,
    /// Commits in the window that touched the folder (owned zones).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub commits: Option<u32>,
    /// Of those, the commits by the owner (owned zones).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub owner_commits: Option<u32>,
    /// Why, in one short phrase.
    pub reason: String,
}

impl ProposedZone {
    /// The zone's glob.
    #[must_use]
    pub fn glob(&self) -> String {
        format!("{}/**", self.folder)
    }
}

/// The whole proposal.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Proposal {
    /// The zones, sorted by folder.
    pub zones: Vec<ProposedZone>,
    /// Commits read (bots left out).
    pub commits: usize,
    /// Distinct authors among them.
    pub authors: usize,
    /// The window, in months.
    pub months: u32,
    /// Why some kinds of zone were not proposed, for the person.
    pub notes: Vec<String>,
}

/// Proposes zones from `commits` for the folders that exist now (`dirs`, see [`tree_dirs`]).
#[must_use]
pub fn propose(commits: &[Commit], dirs: &BTreeSet<String>, thresholds: &Thresholds) -> Proposal {
    let mut zones = Vec::new();
    let mut open: Vec<&str> = Vec::new();
    for dir in dirs {
        let name = dir.rsplit('/').next().unwrap_or(dir);
        let under_open = open.iter().any(|o| is_under(dir, o));
        if !under_open
            && OPEN_FOLDER_NAMES
                .iter()
                .any(|n| n.eq_ignore_ascii_case(name))
        {
            open.push(dir);
            zones.push(ProposedZone {
                folder: dir.clone(),
                kind: ZoneKind::Open,
                owners: Vec::new(),
                commits: None,
                owner_commits: None,
                reason: "folder name".to_owned(),
            });
        }
    }

    // Commits per folder per author: each commit counts once for each folder it touched.
    let mut per_dir: HashMap<&str, HashMap<&str, u32>> = HashMap::new();
    let mut authors = BTreeSet::new();
    for commit in commits {
        authors.insert(commit.author.as_str());
        let mut touched = BTreeSet::new();
        for file in &commit.files {
            let mut parts = file.split('/');
            let (Some(first), Some(second)) = (parts.next(), parts.next()) else {
                continue; // a file at the root
            };
            let top_len = first.len();
            touched.insert(&file[..top_len]);
            if parts.next().is_some() {
                touched.insert(&file[..top_len + 1 + second.len()]);
            }
        }
        for dir in touched {
            *per_dir
                .entry(dir)
                .or_default()
                .entry(commit.author.as_str())
                .or_default() += 1;
        }
    }

    let mut notes = Vec::new();
    if commits.is_empty() {
        notes.push(format!(
            "No commits in the last {} months, so no folder is proposed as owned.",
            thresholds.months
        ));
    } else if authors.len() < 2 {
        notes.push(format!(
            "Only one author in the last {} months, so no folder is proposed as owned.",
            thresholds.months
        ));
    } else {
        let owned = |dir: &str| -> Option<ProposedZone> {
            let counts = per_dir.get(dir)?;
            let total: u32 = counts.values().sum();
            // The top author; ties go to the smaller email so the result is stable.
            let (owner, mine) = counts
                .iter()
                .max_by(|a, b| a.1.cmp(b.1).then_with(|| b.0.cmp(a.0)))?;
            let enough = total >= thresholds.min_commits
                && u64::from(*mine) * 100
                    >= u64::from(thresholds.min_share_percent) * u64::from(total);
            let valid = Owner::try_from((*owner).to_owned()).is_ok();
            (enough && valid).then(|| ProposedZone {
                folder: dir.to_owned(),
                kind: ZoneKind::Owned,
                owners: vec![(*owner).to_owned()],
                commits: Some(total),
                owner_commits: Some(*mine),
                reason: format!(
                    "{mine} of {total} commits in the last {} months",
                    thresholds.months
                ),
            })
        };
        for top in dirs.iter().filter(|d| !d.contains('/')) {
            if open.iter().any(|o| is_under(top, o)) {
                continue;
            }
            if let Some(zone) = owned(top) {
                zones.push(zone);
                continue;
            }
            for child in dirs.iter().filter(|d| parent(d) == Some(top.as_str())) {
                if !open.iter().any(|o| is_under(child, o))
                    && let Some(zone) = owned(child)
                {
                    zones.push(zone);
                }
            }
        }
    }

    zones.sort_by(|a, b| a.folder.cmp(&b.folder));
    Proposal {
        zones,
        commits: commits.len(),
        authors: authors.len(),
        months: thresholds.months,
        notes,
    }
}

fn parent(dir: &str) -> Option<&str> {
    dir.rsplit_once('/').map(|(parent, _)| parent)
}

fn is_under(dir: &str, ancestor: &str) -> bool {
    dir == ancestor
        || dir
            .strip_prefix(ancestor)
            .is_some_and(|rest| rest.starts_with('/'))
}

/// Whether a folder name can be written as a glob without escaping.
fn plain_name(name: &str) -> bool {
    !name.is_empty()
        && !name.contains(['*', '?', '[', ']', '{', '}', '\\', '!'])
        && !name.chars().any(char::is_control)
}

fn git(work_tree: &Path) -> Command {
    let mut command = Command::new("git");
    command.current_dir(work_tree);
    for var in [
        "GIT_DIR",
        "GIT_WORK_TREE",
        "GIT_INDEX_FILE",
        "GIT_COMMON_DIR",
        "GIT_PREFIX",
    ] {
        command.env_remove(var);
    }
    command.stdin(Stdio::null()).stderr(Stdio::null());
    command
}

/// Whether `HEAD` points at a commit.
fn has_commits(work_tree: &Path) -> bool {
    git(work_tree)
        .args(["rev-parse", "--verify", "--quiet", "HEAD"])
        .stdout(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

/// The commits of the last `months` months on `HEAD`, without merges and bots, newest first, at
/// most [`MAX_COMMITS`]. Empty when the repository has no commits.
///
/// # Errors
///
/// A message when `git log` cannot run or fails.
pub fn history(work_tree: &Path, months: u32) -> Result<Vec<Commit>, String> {
    if !has_commits(work_tree) {
        return Ok(Vec::new());
    }
    let out = git(work_tree)
        .args([
            "-c",
            "log.showSignature=false",
            "log",
            "-z",
            "--no-merges",
            "--no-renames",
            "--name-only",
            "--format=%x1e%aE%x1f%aN",
        ])
        .arg(format!("--since={months} months ago"))
        .arg(format!("--max-count={MAX_COMMITS}"))
        .arg("HEAD")
        .output()
        .map_err(|err| format!("cannot run git: {err}"))?;
    if !out.status.success() {
        return Err("git log failed".to_owned());
    }
    Ok(parse_log(&String::from_utf8_lossy(&out.stdout)))
}

/// Parses the `git log -z --name-only --format=%x1e%aE%x1f%aN` output of [`history`].
#[must_use]
pub fn parse_log(text: &str) -> Vec<Commit> {
    let mut commits = Vec::new();
    for record in text.split('\x1e').filter(|r| !r.is_empty()) {
        let (header, files) = record.split_once('\0').unwrap_or((record, ""));
        let (email, name) = header.split_once('\x1f').unwrap_or((header, ""));
        if is_bot(email, name) {
            continue;
        }
        let files = files
            .split('\0')
            .map(|f| f.trim_start_matches('\n'))
            .filter(|f| !f.is_empty())
            .map(str::to_owned)
            .collect();
        commits.push(Commit {
            author: email.trim().to_lowercase(),
            files,
        });
    }
    commits
}

/// Authors that are not people: `[bot]` accounts and `noreply@` / `no-reply@` addresses (an
/// agent committing under its vendor's address, for example). GitHub's per-user
/// `<id>+<name>@users.noreply.github.com` addresses are people and are kept.
fn is_bot(email: &str, name: &str) -> bool {
    let local = email.trim().split('@').next().unwrap_or("").to_lowercase();
    email.contains("[bot]") || name.contains("[bot]") || local == "noreply" || local == "no-reply"
}

/// The folders at depth one and two in `HEAD`'s tree whose names can be written as plain globs.
/// Empty when the repository has no commits.
///
/// # Errors
///
/// A message when `git ls-tree` cannot run or fails.
pub fn tree_dirs(work_tree: &Path) -> Result<BTreeSet<String>, String> {
    if !has_commits(work_tree) {
        return Ok(BTreeSet::new());
    }
    let mut dirs = BTreeSet::new();
    let tops = ls_dirs(work_tree, None)?;
    for top in tops {
        for child in ls_dirs(work_tree, Some(&top))? {
            dirs.insert(format!("{top}/{child}"));
        }
        dirs.insert(top);
    }
    Ok(dirs)
}

fn ls_dirs(work_tree: &Path, under: Option<&str>) -> Result<Vec<String>, String> {
    let mut command = git(work_tree);
    command.args(["ls-tree", "-d", "-z", "--name-only", "--full-tree"]);
    match under {
        Some(dir) => command.arg(format!("HEAD:{dir}")),
        None => command.arg("HEAD"),
    };
    let out = command
        .output()
        .map_err(|err| format!("cannot run git: {err}"))?;
    if !out.status.success() {
        return Err("git ls-tree failed".to_owned());
    }
    Ok(String::from_utf8_lossy(&out.stdout)
        .split('\0')
        .filter(|name| plain_name(name))
        .map(str::to_owned)
        .collect())
}

/// Reads git and proposes (both steps of this module).
///
/// # Errors
///
/// A message when git cannot be read.
pub fn propose_for(work_tree: &Path, thresholds: &Thresholds) -> Result<Proposal, String> {
    let commits = history(work_tree, thresholds.months)?;
    let dirs = tree_dirs(work_tree)?;
    Ok(propose(&commits, &dirs, thresholds))
}

/// A TOML basic string.
fn toml_str(text: &str) -> String {
    let mut out = String::from("\"");
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if c.is_control() => {
                let _ = write!(out, "\\u{:04X}", u32::from(c));
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// `territory.toml` holding `proposal`'s zones, under `header` (the template's opening comment).
#[must_use]
pub fn territory_toml(header: &str, proposal: &Proposal) -> String {
    let mut out = header.trim_end().to_owned();
    let _ = write!(
        out,
        "\n#\n# Proposed by `acs init` from folder names and the last {} months of git history. It is\n\
         # only a starting point: edit it freely. ACS does not enforce `owned` zones yet; until\n\
         # it does they act like `shared`.\n",
        proposal.months
    );
    let mut by_kind: BTreeMap<u8, Vec<&ProposedZone>> = BTreeMap::new();
    for zone in &proposal.zones {
        let order = match zone.kind {
            ZoneKind::Open => 0,
            _ => 1,
        };
        by_kind.entry(order).or_default().push(zone);
    }
    for zone in by_kind.values().flatten() {
        let kind = match zone.kind {
            ZoneKind::Owned => "owned",
            ZoneKind::Shared => "shared",
            ZoneKind::Open => "open",
            ZoneKind::Sealed => "sealed",
        };
        let _ = write!(
            out,
            "\n# {}/: {}\n[[zone]]\npaths = [{}]\nkind = \"{kind}\"\n",
            zone.folder,
            zone.reason,
            toml_str(&zone.glob())
        );
        if !zone.owners.is_empty() {
            let owners: Vec<String> = zone.owners.iter().map(|o| toml_str(o)).collect();
            let _ = writeln!(out, "owners = [{}]", owners.join(", "));
        }
    }
    out.push_str(
        "\n# Teams are referenced from `owners` as \"team:<name>\".\n\
         # [teams]\n# platform = [\"omar@example.com\", \"lina@example.com\"]\n",
    );
    out
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    fn commit(author: &str, files: &[&str]) -> Commit {
        Commit {
            author: author.to_owned(),
            files: files.iter().map(|f| (*f).to_owned()).collect(),
        }
    }

    fn dirs(list: &[&str]) -> BTreeSet<String> {
        list.iter().map(|d| (*d).to_owned()).collect()
    }

    fn repeat(n: usize, author: &str, files: &[&str]) -> Vec<Commit> {
        (0..n).map(|_| commit(author, files)).collect()
    }

    fn summary(p: &Proposal) -> Vec<String> {
        p.zones
            .iter()
            .map(|z| format!("{:?} {} {}", z.kind, z.folder, z.owners.join(",")))
            .collect()
    }

    #[test]
    fn a_clear_majority_owns_a_folder_and_mixed_folders_stay_shared() {
        let mut commits = repeat(9, "sara@x.com", &["payments/a.rs"]);
        commits.extend(repeat(1, "omar@x.com", &["payments/b.rs"]));
        commits.extend(repeat(5, "sara@x.com", &["lib/a.rs"]));
        commits.extend(repeat(5, "omar@x.com", &["lib/b.rs"]));
        let p = propose(
            &commits,
            &dirs(&["payments", "lib"]),
            &Thresholds::default(),
        );
        assert_eq!(summary(&p), ["Owned payments sara@x.com"]);
        assert_eq!(p.zones[0].commits, Some(10));
        assert_eq!(p.zones[0].owner_commits, Some(9));
        assert_eq!(p.authors, 2);
    }

    #[test]
    fn too_few_commits_or_too_small_a_share_owns_nothing() {
        let mut commits = repeat(8, "sara@x.com", &["a/x"]);
        commits.extend(repeat(1, "omar@x.com", &["b/x"]));
        // a/: 8 commits, under the minimum of 10.
        let p = propose(&commits, &dirs(&["a", "b"]), &Thresholds::default());
        assert!(p.zones.is_empty(), "{:?}", summary(&p));

        let mut commits = repeat(7, "sara@x.com", &["a/x"]);
        commits.extend(repeat(3, "omar@x.com", &["a/y"]));
        // 70% is under 75%.
        let p = propose(&commits, &dirs(&["a"]), &Thresholds::default());
        assert!(p.zones.is_empty());
        let lower = Thresholds {
            min_share_percent: 70,
            ..Thresholds::default()
        };
        assert_eq!(
            summary(&propose(&commits, &dirs(&["a"]), &lower)),
            ["Owned a sara@x.com"]
        );
    }

    #[test]
    fn a_mixed_top_folder_can_have_owned_subfolders() {
        let mut commits = repeat(10, "sara@x.com", &["crates/pay/src/a.rs"]);
        commits.extend(repeat(10, "omar@x.com", &["crates/web/b.rs"]));
        let p = propose(
            &commits,
            &dirs(&["crates", "crates/pay", "crates/web"]),
            &Thresholds::default(),
        );
        assert_eq!(
            summary(&p),
            ["Owned crates/pay sara@x.com", "Owned crates/web omar@x.com"]
        );
    }

    #[test]
    fn an_owned_top_folder_hides_its_subfolders() {
        let mut commits = repeat(10, "sara@x.com", &["pay/core/a.rs"]);
        commits.extend(repeat(1, "omar@x.com", &["other/x"]));
        let p = propose(
            &commits,
            &dirs(&["pay", "pay/core", "other"]),
            &Thresholds::default(),
        );
        assert_eq!(summary(&p), ["Owned pay sara@x.com"]);
    }

    #[test]
    fn one_author_owns_nothing() {
        let commits = repeat(30, "sara@x.com", &["src/a.rs"]);
        let p = propose(&commits, &dirs(&["src"]), &Thresholds::default());
        assert!(p.zones.is_empty());
        assert!(p.notes[0].contains("Only one author"), "{:?}", p.notes);
    }

    #[test]
    fn folder_names_are_open_and_never_owned() {
        let mut commits = repeat(10, "sara@x.com", &["docs/a.md"]);
        commits.extend(repeat(10, "omar@x.com", &["src/a.rs"]));
        let p = propose(
            &commits,
            &dirs(&["docs", "docs/api", "src", "src/Examples", "tools"]),
            &Thresholds::default(),
        );
        assert_eq!(
            summary(&p),
            ["Open docs ", "Owned src omar@x.com", "Open src/Examples "]
        );
    }

    #[test]
    fn folders_gone_from_the_tree_are_not_proposed() {
        let mut commits = repeat(10, "sara@x.com", &["old/a.rs"]);
        commits.extend(repeat(1, "omar@x.com", &["x/b"]));
        let p = propose(&commits, &dirs(&["x"]), &Thresholds::default());
        assert!(p.zones.is_empty());
    }

    #[test]
    fn root_files_count_for_no_folder() {
        let mut commits = repeat(10, "sara@x.com", &["README.md"]);
        commits.extend(repeat(1, "omar@x.com", &["README.md"]));
        let p = propose(&commits, &dirs(&["src"]), &Thresholds::default());
        assert!(p.zones.is_empty());
    }

    #[test]
    fn the_log_parser_reads_records_and_drops_bots() {
        let text = "\x1eSara@X.com\x1fSara\0\na/b/x\0c d/é\0\
                    \x1edependabot[bot]@users.noreply.github.com\x1fdependabot[bot]\0\na/y\0\
                    \x1eNoReply@vendor.example\x1fAgent\0\na/z\0\
                    \x1e1+sam@users.noreply.github.com\x1fSam\0\na/w\0\
                    \x1eomar@x.com\x1fOmar\0";
        let commits = parse_log(text);
        assert_eq!(
            commits,
            [
                commit("sara@x.com", &["a/b/x", "c d/é"]),
                commit("1+sam@users.noreply.github.com", &["a/w"]),
                commit("omar@x.com", &[]),
            ]
        );
    }

    #[test]
    fn the_territory_file_is_valid_policy_with_the_zones() {
        let mut commits = repeat(10, "sara@x.com", &["pay/a.rs"]);
        commits.extend(repeat(1, "omar@x.com", &["x/b"]));
        let p = propose(
            &commits,
            &dirs(&["pay", "docs", "x", "we \"ird"]),
            &Thresholds::default(),
        );
        let text = territory_toml("# header", &p);
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("territory.toml"), &text).unwrap();
        let loaded = acs_policy::load(dir.path(), acs_policy::LoadOptions::default()).unwrap();
        assert!(loaded.warnings.is_empty(), "{:?}", loaded.warnings);
        let zones = &loaded.policy.files().territory.zone;
        assert_eq!(zones.len(), 2, "{text}");
        assert_eq!(zones[0].paths, ["docs/**"]);
        assert_eq!(zones[0].kind, ZoneKind::Open);
        assert_eq!(zones[1].paths, ["pay/**"]);
        assert_eq!(zones[1].owners, [Owner::Pilot("sara@x.com".into())]);
    }

    #[test]
    fn toml_strings_escape_quotes_and_backslashes() {
        assert_eq!(toml_str(r#"a"b\c"#), r#""a\"b\\c""#);
    }
}
