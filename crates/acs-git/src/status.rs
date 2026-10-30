//! Working-tree change detection for post-tool attribution of shell commands.
//!
//! After a shell command the hook needs to know which files the command changed. Two strategies,
//! chosen by the size of the repository ([`ScanMode`]):
//!
//! - **Full** (up to [`ScanOptions::full_scan_max_entries`] index entries): ask git. One
//!   `git status --porcelain=v2 -z --untracked-files=all` lists every modified, deleted and
//!   untracked file that is not ignored. Each is then `stat`ed and kept if it changed at or
//!   after the command started. Complete, and it respects `.gitignore` exactly.
//! - **Scoped** (larger repositories, where a full `git status` alone exceeds the post-tool
//!   budget; plan section 9.2 names this fallback): look only at the files the caller names (the
//!   command's predicted targets and the session's claimed files) and at the entries of a few
//!   directories (where the command ran, where the targets live), without recursion. Candidates
//!   not named explicitly are filtered through `git check-ignore`, started only when there are
//!   any. Files changed elsewhere by an opaque script are **not** seen in this mode.
//!
//! "Changed by this command" means: modification or status-change time at or after `since` (the
//! time the pre-tool step recorded), with a little slack for timestamp granularity. Files other
//! processes touched during the command are indistinguishable from the command's own changes.
//! Nothing here hashes content: the caller hashes the few files reported.

use std::collections::HashSet;
use std::fs;
use std::io::Write;
use std::path::Path;
use std::process::Stdio;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::GitError;
use crate::repo::{RepoLayout, git_command, index_entry_count};

/// Which strategy to use.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ScanMode {
    /// Full when the index is small enough, scoped otherwise.
    #[default]
    Auto,
    /// Always ask git for the whole work tree.
    Full,
    /// Only the named files and directories.
    Scoped,
}

/// Tuning of a scan.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScanOptions {
    /// The strategy; [`ScanMode::Auto`] by default.
    pub mode: ScanMode,
    /// With [`ScanMode::Auto`], repositories with more index entries than this are scanned in
    /// scoped mode.
    pub full_scan_max_entries: u32,
    /// The most candidate files examined per scan; beyond it the report is marked truncated.
    pub max_candidates: usize,
}

impl Default for ScanOptions {
    fn default() -> Self {
        Self {
            mode: ScanMode::Auto,
            full_scan_max_entries: DEFAULT_FULL_SCAN_MAX_ENTRIES,
            max_candidates: 5_000,
        }
    }
}

/// The default of [`ScanOptions::full_scan_max_entries`]; see decision 0012 for the
/// measurement behind it.
pub const DEFAULT_FULL_SCAN_MAX_ENTRIES: u32 = 20_000;

/// What to look at.
#[derive(Debug, Clone, Copy)]
pub struct ScanInput<'a> {
    /// Files changed at or after this time count as changed.
    pub since: SystemTime,
    /// Repository-relative, `/`-separated paths that are always examined (in both modes): the
    /// command's predicted targets, the session's claimed files.
    pub known_paths: &'a [String],
    /// Repository-relative directories (`""` is the root) whose entries are listed, without
    /// recursion, in scoped mode.
    pub scope_dirs: &'a [String],
}

/// Whether a changed path exists now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChangeKind {
    /// The path exists and changed at or after `since` (it is new or modified).
    Present,
    /// A path from `known_paths` that does not exist.
    Missing,
}

/// One changed path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Change {
    /// Repository-relative, `/`-separated.
    pub path: String,
    /// Whether it exists now.
    pub kind: ChangeKind,
}

/// The result of a scan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScanReport {
    /// The strategy that was used.
    pub mode: ScanMode,
    /// Changed paths, sorted.
    pub changes: Vec<Change>,
    /// Full mode only: every tracked file that is missing from the work tree right now. The caller
    /// diffs this against the previous scan to find deletions made by the command (git cannot
    /// say when a file disappeared).
    pub deleted_tracked: Vec<String>,
    /// More candidates existed than [`ScanOptions::max_candidates`]; some were not examined.
    pub truncated: bool,
}

/// Scans for files changed since `input.since`.
///
/// # Errors
///
/// [`GitError::Command`] when `git status` cannot be run in full mode. Scoped mode never fails:
/// unreadable entries are skipped.
pub fn scan_changes(
    layout: &RepoLayout,
    input: &ScanInput<'_>,
    options: &ScanOptions,
) -> Result<ScanReport, GitError> {
    let mode = match options.mode {
        ScanMode::Auto => match index_entry_count(&layout.git_dir) {
            Some(count) if count > options.full_scan_max_entries => ScanMode::Scoped,
            _ => ScanMode::Full,
        },
        explicit => explicit,
    };
    let mut report = match mode {
        ScanMode::Scoped => scoped(layout, input, options),
        _ => full(layout, input, options)?,
    };
    report.changes.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(report)
}

fn full(
    layout: &RepoLayout,
    input: &ScanInput<'_>,
    options: &ScanOptions,
) -> Result<ScanReport, GitError> {
    let output = git_command(&layout.work_tree)
        .args([
            "status",
            "--porcelain=v2",
            "-z",
            "--untracked-files=all",
            "--no-renames",
        ])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .map_err(|e| GitError::Command {
            args: "status".to_owned(),
            detail: e.to_string(),
        })?;
    if !output.status.success() {
        return Err(GitError::Command {
            args: "status".to_owned(),
            detail: format!("exited with {}", output.status),
        });
    }
    let parsed = parse_status(&output.stdout);
    let mut candidates: Vec<String> = parsed.candidates;
    let truncated = candidates.len() > options.max_candidates;
    candidates.truncate(options.max_candidates);
    let known: HashSet<&str> = input.known_paths.iter().map(String::as_str).collect();
    let mut changes = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    for path in candidates
        .iter()
        .map(String::as_str)
        .chain(input.known_paths.iter().map(String::as_str))
    {
        if !seen.insert(path.to_owned()) {
            continue;
        }
        // A known path that is missing is reported here even when it is also in
        // `deleted_tracked`: the caller named it, so the deletion is certain to be relevant.
        if let Some(change) = examine(&layout.work_tree, path, input.since, known.contains(path)) {
            changes.push(change);
        }
    }
    Ok(ScanReport {
        mode: ScanMode::Full,
        changes,
        deleted_tracked: parsed.deleted,
        truncated,
    })
}

fn scoped(layout: &RepoLayout, input: &ScanInput<'_>, options: &ScanOptions) -> ScanReport {
    let root = &layout.work_tree;
    let mut changes = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    for path in input.known_paths {
        if seen.insert(path.clone())
            && let Some(change) = examine(root, path, input.since, true)
        {
            changes.push(change);
        }
    }
    let mut found: Vec<String> = Vec::new();
    let mut examined = 0usize;
    let mut truncated = false;
    'dirs: for dir in input.scope_dirs {
        if !seen.insert(format!("{dir}/")) {
            continue;
        }
        let Ok(entries) = fs::read_dir(root.join(dir)) else {
            continue;
        };
        for entry in entries.flatten() {
            examined += 1;
            if examined > options.max_candidates {
                truncated = true;
                break 'dirs;
            }
            let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
                continue;
            };
            if name == ".git" {
                continue;
            }
            let Ok(meta) = entry.metadata() else { continue };
            if meta.is_dir() || !changed_since(&meta, input.since) {
                continue;
            }
            let path = if dir.is_empty() {
                name
            } else {
                format!("{dir}/{name}")
            };
            if seen.insert(path.clone()) {
                found.push(path);
            }
        }
    }
    let ignored = ignored_among(root, &found);
    for path in found {
        if !ignored.contains(&path) {
            changes.push(Change {
                path,
                kind: ChangeKind::Present,
            });
        }
    }
    ScanReport {
        mode: ScanMode::Scoped,
        changes,
        deleted_tracked: Vec::new(),
        truncated,
    }
}

/// Stats one path and reports it when it changed since `since` (or, for a known path, when it is
/// missing).
fn examine(root: &Path, path: &str, since: SystemTime, known: bool) -> Option<Change> {
    match fs::symlink_metadata(root.join(path)) {
        Ok(meta) if meta.is_dir() => None,
        Ok(meta) => changed_since(&meta, since).then(|| Change {
            path: path.to_owned(),
            kind: ChangeKind::Present,
        }),
        Err(_) => known.then(|| Change {
            path: path.to_owned(),
            kind: ChangeKind::Missing,
        }),
    }
}

/// Whether the file's modification or status-change time is at or after `since`, allowing for
/// timestamp granularity: the kernel's coarse clock lags a few milliseconds, and some
/// filesystems store whole seconds.
fn changed_since(meta: &fs::Metadata, since: SystemTime) -> bool {
    let Ok(mtime) = meta.modified() else {
        return true; // Unknown: better to over-attribute than to miss a change.
    };
    let coarse = mtime
        .duration_since(UNIX_EPOCH)
        .is_ok_and(|d| d.subsec_nanos() == 0);
    let slack = if coarse {
        Duration::from_secs(2)
    } else {
        Duration::from_millis(20)
    };
    let threshold = since.checked_sub(slack).unwrap_or(UNIX_EPOCH);
    if mtime >= threshold {
        return true;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        // A rename or chmod changes the status-change time but not the modification time.
        let ctime = UNIX_EPOCH + Duration::new(meta.ctime().max(0).cast_unsigned(), 0);
        let ctime = ctime + Duration::from_nanos(meta.ctime_nsec().max(0).cast_unsigned());
        if ctime >= threshold {
            return true;
        }
    }
    false
}

/// The paths among `paths` that `.gitignore` rules ignore, via `git check-ignore`. Fails open:
/// if git cannot answer, nothing is considered ignored.
fn ignored_among(root: &Path, paths: &[String]) -> HashSet<String> {
    if paths.is_empty() {
        return HashSet::new();
    }
    let Ok(mut child) = git_command(root)
        // `--no-index`: do not read the index to learn what is tracked (a read of the whole index,
        // tens of milliseconds on a huge repository). A tracked file that matches an ignore
        // pattern (force-added) is therefore treated as ignored.
        .args(["check-ignore", "--no-index", "-z", "--stdin"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
    else {
        return HashSet::new();
    };
    let mut input = Vec::new();
    for path in paths {
        input.extend_from_slice(path.as_bytes());
        input.push(0);
    }
    // Feed the paths from a second thread while this one drains the answers: with thousands of
    // ignored paths both pipes can fill, and doing one after the other would deadlock.
    let stdin = child.stdin.take();
    let Ok(output) = std::thread::scope(|scope| {
        scope.spawn(move || {
            if let Some(mut stdin) = stdin {
                let _ = stdin.write_all(&input);
            }
        });
        child.wait_with_output()
    }) else {
        return HashSet::new();
    };
    output
        .stdout
        .split(|&b| b == 0)
        .filter(|p| !p.is_empty())
        .map(|p| String::from_utf8_lossy(p).into_owned())
        .collect()
}

/// What `git status --porcelain=v2 -z` reported.
#[derive(Debug, Default, PartialEq, Eq)]
struct Parsed {
    /// Modified, added and untracked paths (candidates for the mtime filter).
    candidates: Vec<String>,
    /// Tracked paths missing from the work tree.
    deleted: Vec<String>,
}

fn parse_status(output: &[u8]) -> Parsed {
    let mut parsed = Parsed::default();
    for record in output.split(|&b| b == 0).filter(|r| !r.is_empty()) {
        let text = String::from_utf8_lossy(record);
        let mut chars = text.chars();
        let (Some(kind), Some(' ')) = (chars.next(), chars.next()) else {
            continue;
        };
        match kind {
            // `1 XY sub mH mI mW hH hI <path>`: the path is the 9th field and may hold spaces.
            '1' => {
                let fields: Vec<&str> = text.splitn(9, ' ').collect();
                if let (Some(xy), Some(path)) = (fields.get(1), fields.get(8)) {
                    if xy.ends_with('D') {
                        parsed.deleted.push((*path).to_owned());
                    } else {
                        parsed.candidates.push((*path).to_owned());
                    }
                }
            }
            // `u XY sub m1 m2 m3 mW h1 h2 h3 <path>`: an unmerged path.
            'u' => {
                let fields: Vec<&str> = text.splitn(11, ' ').collect();
                if let Some(path) = fields.get(10) {
                    parsed.candidates.push((*path).to_owned());
                }
            }
            '?' => parsed.candidates.push(text[2..].to_owned()),
            _ => {}
        }
    }
    parsed
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::repo::discover;

    fn git(dir: &Path, args: &[&str]) {
        let status = git_command(dir)
            .args(["-c", "user.email=t@example.com", "-c", "user.name=t"])
            .args(args)
            .stdout(Stdio::null())
            .status()
            .unwrap();
        assert!(status.success(), "git {args:?}");
    }

    struct Fixture {
        _tmp: tempfile::TempDir,
        root: PathBuf,
        layout: RepoLayout,
    }

    fn fixture() -> Fixture {
        let tmp = tempfile::tempdir().unwrap();
        let root = dunce::canonicalize(tmp.path()).unwrap();
        git(&root, &["init", "-q", "-b", "main", "."]);
        fs::write(root.join(".gitignore"), "target/\n*.log\n").unwrap();
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(root.join("src/a.rs"), "a\n").unwrap();
        fs::write(root.join("src/b.rs"), "b\n").unwrap();
        fs::write(root.join("README.md"), "readme\n").unwrap();
        git(&root, &["add", "-A"]);
        git(&root, &["commit", "-q", "-m", "init"]);
        let layout = discover(&root).unwrap();
        // Files written by the fixture are old news.
        std::thread::sleep(Duration::from_millis(60));
        Fixture {
            _tmp: tmp,
            root,
            layout,
        }
    }

    fn paths(report: &ScanReport) -> Vec<(&str, ChangeKind)> {
        report
            .changes
            .iter()
            .map(|c| (c.path.as_str(), c.kind))
            .collect()
    }

    fn opts(mode: ScanMode) -> ScanOptions {
        ScanOptions {
            mode,
            ..ScanOptions::default()
        }
    }

    #[test]
    fn full_scan_finds_modified_new_and_ignores_ignored() {
        let f = fixture();
        let since = SystemTime::now();
        std::thread::sleep(Duration::from_millis(60));
        fs::write(f.root.join("src/a.rs"), "changed\n").unwrap();
        fs::write(f.root.join("src/new.rs"), "new\n").unwrap();
        fs::create_dir_all(f.root.join("target/debug")).unwrap();
        fs::write(f.root.join("target/debug/out"), "built\n").unwrap();
        fs::write(f.root.join("run.log"), "log\n").unwrap();
        let report = scan_changes(
            &f.layout,
            &ScanInput {
                since,
                known_paths: &[],
                scope_dirs: &[],
            },
            &opts(ScanMode::Full),
        )
        .unwrap();
        assert_eq!(report.mode, ScanMode::Full);
        assert_eq!(
            paths(&report),
            vec![
                ("src/a.rs", ChangeKind::Present),
                ("src/new.rs", ChangeKind::Present)
            ]
        );
        assert!(report.deleted_tracked.is_empty());
    }

    #[test]
    fn files_dirty_before_the_command_are_not_attributed_to_it() {
        let f = fixture();
        fs::write(f.root.join("src/b.rs"), "dirty before\n").unwrap();
        std::thread::sleep(Duration::from_millis(60));
        let since = SystemTime::now();
        std::thread::sleep(Duration::from_millis(60));
        fs::write(f.root.join("src/a.rs"), "changed\n").unwrap();
        let report = scan_changes(
            &f.layout,
            &ScanInput {
                since,
                known_paths: &[],
                scope_dirs: &[],
            },
            &opts(ScanMode::Full),
        )
        .unwrap();
        assert_eq!(paths(&report), vec![("src/a.rs", ChangeKind::Present)]);
    }

    #[test]
    fn full_scan_reports_tracked_deletions_separately() {
        let f = fixture();
        let since = SystemTime::now();
        fs::remove_file(f.root.join("src/b.rs")).unwrap();
        let report = scan_changes(
            &f.layout,
            &ScanInput {
                since,
                known_paths: &[],
                scope_dirs: &[],
            },
            &opts(ScanMode::Full),
        )
        .unwrap();
        assert_eq!(report.deleted_tracked, vec!["src/b.rs".to_owned()]);
        assert!(report.changes.is_empty());
        // A known path that is missing is a change of its own kind.
        let known = vec!["never/existed.rs".to_owned()];
        let report = scan_changes(
            &f.layout,
            &ScanInput {
                since,
                known_paths: &known,
                scope_dirs: &[],
            },
            &opts(ScanMode::Full),
        )
        .unwrap();
        assert_eq!(
            paths(&report),
            vec![("never/existed.rs", ChangeKind::Missing)]
        );
    }

    #[test]
    fn scoped_scan_sees_known_paths_and_listed_directories_only() {
        let f = fixture();
        let since = SystemTime::now();
        std::thread::sleep(Duration::from_millis(60));
        fs::write(f.root.join("README.md"), "edited\n").unwrap();
        fs::write(f.root.join("src/new.rs"), "new\n").unwrap();
        fs::write(f.root.join("src/debug.log"), "ignored\n").unwrap();
        fs::create_dir_all(f.root.join("elsewhere")).unwrap();
        fs::write(f.root.join("elsewhere/hidden.rs"), "opaque\n").unwrap();
        let known = vec!["README.md".to_owned(), "gone.rs".to_owned()];
        let dirs = vec!["src".to_owned()];
        let report = scan_changes(
            &f.layout,
            &ScanInput {
                since,
                known_paths: &known,
                scope_dirs: &dirs,
            },
            &opts(ScanMode::Scoped),
        )
        .unwrap();
        assert_eq!(report.mode, ScanMode::Scoped);
        assert_eq!(
            paths(&report),
            vec![
                ("README.md", ChangeKind::Present),
                ("gone.rs", ChangeKind::Missing),
                ("src/new.rs", ChangeKind::Present),
            ],
            "the ignored log is filtered, the unlisted directory is not seen"
        );
    }

    #[test]
    fn auto_mode_picks_scoped_for_big_indexes() {
        let f = fixture();
        let since = SystemTime::now();
        let input = ScanInput {
            since,
            known_paths: &[],
            scope_dirs: &[],
        };
        let small = scan_changes(&f.layout, &input, &ScanOptions::default()).unwrap();
        assert_eq!(small.mode, ScanMode::Full);
        let tiny_limit = ScanOptions {
            full_scan_max_entries: 1,
            ..ScanOptions::default()
        };
        let big = scan_changes(&f.layout, &input, &tiny_limit).unwrap();
        assert_eq!(big.mode, ScanMode::Scoped);
    }

    #[test]
    fn status_output_parsing() {
        let out = b"1 .M N... 100644 100644 100644 aaa bbb src/with space.rs\0\
                    1 .D N... 100644 100644 000000 aaa bbb gone.rs\0\
                    ? new.txt\0\
                    u UU N... 100644 100644 100644 100644 a b c conflicted.rs\0";
        let parsed = parse_status(out);
        assert_eq!(
            parsed.candidates,
            vec!["src/with space.rs", "new.txt", "conflicted.rs"]
        );
        assert_eq!(parsed.deleted, vec!["gone.rs"]);
    }
}
