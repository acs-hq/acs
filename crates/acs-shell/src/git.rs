//! `git` invocations: global options, subcommand classification and the facts the policy needs
//! (history rewrites, force pushes, commits, named refs).

use acs_core::gate::{CommandClass, GitInvocation, TargetOp};

use crate::args::{Outcome, Parsed};
use crate::word::{Kind, Word, basename};

/// Subcommands that only read the repository.
const READ_ONLY: &[&str] = &[
    "status",
    "log",
    "diff",
    "show",
    "blame",
    "annotate",
    "grep",
    "ls-files",
    "ls-tree",
    "rev-parse",
    "rev-list",
    "describe",
    "shortlog",
    "cat-file",
    "show-ref",
    "for-each-ref",
    "name-rev",
    "merge-base",
    "whatchanged",
    "count-objects",
    "fsck",
    "verify-commit",
    "verify-tag",
    "verify-pack",
    "help",
    "version",
    "check-ignore",
    "check-attr",
    "diff-tree",
    "diff-index",
    "diff-files",
    "cherry",
    "range-diff",
    "show-branch",
    "var",
    "difftool",
    "request-pull",
    "get-tar-commit-id",
];

/// Options of `git` itself (before the subcommand) that take a value as the next word.
const GLOBAL_VALUE_OPTS: &[&str] = &[
    "-C",
    "-c",
    "--git-dir",
    "--work-tree",
    "--namespace",
    "--super-prefix",
    "--config-env",
];

/// Classifies `git <args>`.
pub(crate) fn classify(args: &[Word], out: &mut Outcome) {
    let mut dirs: Vec<Word> = Vec::new();
    let mut i = 0;
    while i < args.len() && args[i].is_option() {
        let name = args[i].text.as_str();
        i += 1;
        match name {
            "-C" | "--work-tree" if i < args.len() => {
                dirs.push(args[i].clone());
                i += 1;
            }
            _ if name.starts_with("--work-tree=") => {
                dirs.push(Word {
                    text: name["--work-tree=".len()..].to_string(),
                    kind: args[i - 1].kind,
                });
            }
            _ if GLOBAL_VALUE_OPTS.contains(&name) => i += 1,
            "--version" | "--help" | "-h" | "--html-path" | "--man-path" | "--info-path"
            | "--exec-path" => {
                out.class(CommandClass::Read);
                out.git = Some(GitInvocation {
                    subcommand: "version".into(),
                    ..GitInvocation::default()
                });
                return;
            }
            _ => {}
        }
    }
    let Some(sub) = args.get(i) else {
        out.class(CommandClass::Read);
        out.git = Some(GitInvocation::default());
        return;
    };
    let rest = &args[i + 1..];
    let mut inv = GitInvocation {
        subcommand: sub.text.clone(),
        ..GitInvocation::default()
    };
    let start = out.targets.len();
    if sub.is_exact() {
        subcommand(&sub.text, rest, out, &mut inv);
    } else {
        out.class(CommandClass::Unknown).unknown_targets();
    }
    for dir in dirs.iter().rev() {
        out.rebase_from(start, dir);
    }
    if !dirs.is_empty() && !dirs.iter().all(Word::is_exact) {
        out.unknown_targets();
    }
    if inv.rewrites_history {
        out.class(CommandClass::GitHistoryRewrite);
    }
    out.git = Some(inv);
}

fn subcommand(sub: &str, rest: &[Word], out: &mut Outcome, inv: &mut GitInvocation) {
    if READ_ONLY.contains(&sub) {
        out.class(CommandClass::Read);
        return;
    }
    match sub {
        "add" | "notes" | "symbolic-ref" | "hash-object" | "update-index" | "write-tree"
        | "mktag" | "pack-refs" | "repack" | "maintenance" | "commit-graph" | "prune-packed" => {
            write_only(sub, rest, out);
        }
        "commit" => commit(rest, out, inv),
        "merge" => {
            let p = Parsed::new(rest, &["-m", "-s", "-X", "-F", "--strategy", "--message"]);
            out.class(CommandClass::Write).unknown_targets();
            inv.creates_commit = !p.has(&["--squash", "--no-commit", "--abort", "--quit"]);
            inv.refs = words_text(p.positionals());
        }
        "cherry-pick" | "revert" => {
            let p = Parsed::new(rest, &["-m", "-s", "-X", "--mainline", "--strategy"]);
            out.class(CommandClass::Write).unknown_targets();
            inv.creates_commit = !p.has(&["-n", "--no-commit", "--abort", "--quit"]);
        }
        "am" => {
            out.class(CommandClass::Write).unknown_targets();
            inv.creates_commit = true;
        }
        "apply" => {
            let p = Parsed::new(rest, &["-p", "-C", "--directory", "--include", "--exclude"]);
            if p.has(&["--check", "--stat", "--numstat", "--summary"]) {
                out.class(CommandClass::Read);
            } else {
                out.class(CommandClass::Write).unknown_targets();
            }
        }
        "rebase" => {
            let p = Parsed::new(
                rest,
                &["-s", "-X", "-x", "--onto", "--exec", "--strategy", "-S"],
            );
            out.class(CommandClass::Write).unknown_targets();
            inv.rewrites_history = true;
            inv.creates_commit = true;
            inv.refs = words_text(p.positionals());
            inv.refs
                .extend(p.values(&["--onto"]).iter().map(|w| w.text.clone()));
        }
        "pull" => pull(rest, out, inv),
        "fetch" | "send-email" | "ls-remote" => {
            out.class(CommandClass::Network);
        }
        "clone" => clone(rest, out),
        "push" => push(rest, out, inv),
        "reset" => reset(rest, out, inv),
        "checkout" => checkout(rest, out, inv),
        "switch" => switch(rest, out, inv),
        "restore" => restore(rest, out, inv),
        "clean" => clean(rest, out, inv),
        "rm" => {
            let p = Parsed::new(rest, &[]);
            if p.has(&["--cached"]) {
                out.class(CommandClass::Write);
            } else {
                out.class(CommandClass::Delete);
                let recursive = p.has(&["-r"]);
                out.targets(p.pos.iter().copied(), TargetOp::Delete, recursive);
            }
        }
        "mv" => {
            let p = Parsed::new(rest, &[]);
            out.class(CommandClass::Move);
            move_targets(&p.pos, p.has(&["-T"]), out);
        }
        "stash" => stash(rest, out, inv),
        "branch" => branch(rest, out, inv),
        "tag" => tag(rest, out, inv),
        "update-ref" => {
            let p = Parsed::new(rest, &["-m", "--create-reflog"]);
            out.class(CommandClass::Write);
            inv.rewrites_history = true;
            inv.refs = words_text(&p.pos[..p.pos.len().min(1)]);
        }
        "filter-branch" | "filter-repo" | "replace" | "fast-import" => {
            out.class(CommandClass::Write).unknown_targets();
            inv.rewrites_history = true;
        }
        "gc" | "prune" => {
            let p = Parsed::new(rest, &[]);
            out.class(CommandClass::Write);
            let prune_now = p
                .opts
                .iter()
                .any(|o| o.name == "--prune" && o.value.as_ref().is_some_and(|v| v.text == "now"));
            inv.rewrites_history = sub == "prune" || prune_now;
        }
        "reflog" => {
            let p = Parsed::new(rest, &[]);
            match p.pos.first().map(|w| w.text.as_str()) {
                Some("expire" | "delete") => {
                    out.class(CommandClass::Write);
                    inv.rewrites_history = true;
                }
                _ => {
                    out.class(CommandClass::Read);
                }
            }
        }
        "config" => config(rest, out),
        "init" => {
            let p = Parsed::new(
                rest,
                &[
                    "--template",
                    "-b",
                    "--initial-branch",
                    "--separate-git-dir",
                    "--object-format",
                ],
            );
            out.class(CommandClass::Write);
            match p.pos.first() {
                Some(dir) => out.target(dir, TargetOp::Write, true),
                None => out.path(".git", Kind::Exact, TargetOp::Write, true),
            };
        }
        "remote" => {
            let p = Parsed::new(rest, &[]);
            match p.pos.first().map(|w| w.text.as_str()) {
                Some(
                    "add" | "remove" | "rm" | "rename" | "set-url" | "set-head" | "set-branches",
                ) => {
                    out.class(CommandClass::Write);
                }
                Some("prune" | "update") => {
                    out.class(CommandClass::Write).class(CommandClass::Network);
                }
                _ => {
                    out.class(CommandClass::Read);
                }
            }
        }
        "submodule" => {
            let p = Parsed::new(rest, &[]);
            match p.pos.first().map(|w| w.text.as_str()) {
                Some("status" | "summary") => {
                    out.class(CommandClass::Read);
                }
                Some("foreach") => {
                    out.and_unknown();
                }
                _ => {
                    out.class(CommandClass::Write)
                        .class(CommandClass::Network)
                        .unknown_targets();
                }
            }
        }
        "worktree" => worktree(rest, out),
        "bisect" => {
            let p = Parsed::new(rest, &[]);
            if p.pos.first().is_some_and(|w| w.text == "run") {
                out.and_unknown();
            }
            out.class(CommandClass::Write).unknown_targets();
        }
        "archive" => {
            let p = Parsed::new(
                rest,
                &["-o", "--output", "--format", "--prefix", "--remote"],
            );
            match p.value(&["-o", "--output"]) {
                Some(file) => {
                    out.class(CommandClass::Write);
                    out.target(&file, TargetOp::Write, false);
                }
                None => {
                    out.class(CommandClass::Read);
                }
            }
        }
        "format-patch" => {
            let p = Parsed::new(
                rest,
                &[
                    "-o",
                    "--output-directory",
                    "--subject-prefix",
                    "--to",
                    "--cc",
                    "--from",
                ],
            );
            if p.has(&["--stdout"]) {
                out.class(CommandClass::Read);
            } else {
                out.class(CommandClass::Write);
                match p.value(&["-o", "--output-directory"]) {
                    Some(dir) => out.target(&dir, TargetOp::Write, true),
                    None => out.unknown_targets(),
                };
            }
        }
        "lfs" => {
            out.class(CommandClass::Network)
                .class(CommandClass::Write)
                .unknown_targets();
        }
        "sparse-checkout" | "read-tree" | "checkout-index" | "mergetool" | "rerere" => {
            out.class(CommandClass::Write).unknown_targets();
        }
        "commit-tree" => {
            out.class(CommandClass::Write);
            inv.creates_commit = true;
        }
        _ => {
            // An alias or a subcommand we do not know: it can do anything.
            out.and_unknown();
        }
    }
}

fn words_text(words: &[&Word]) -> Vec<String> {
    words.iter().map(|w| w.text.clone()).collect()
}

/// Commands that change repository state but no working-tree file.
fn write_only(sub: &str, rest: &[Word], out: &mut Outcome) {
    let p = Parsed::new(rest, &[]);
    if sub == "notes"
        && matches!(
            p.pos.first().map(|w| w.text.as_str()),
            None | Some("list" | "show")
        )
    {
        out.class(CommandClass::Read);
    } else {
        out.class(CommandClass::Write);
    }
}

fn commit(rest: &[Word], out: &mut Outcome, inv: &mut GitInvocation) {
    let p = Parsed::new(
        rest,
        &[
            "-m",
            "-F",
            "-C",
            "-c",
            "-t",
            "--message",
            "--file",
            "--author",
            "--date",
            "--reuse-message",
            "--reedit-message",
            "--template",
            "--cleanup",
            "--fixup",
            "--squash",
            "-S",
            "--gpg-sign",
        ],
    );
    out.class(CommandClass::Write);
    inv.creates_commit = true;
    inv.rewrites_history = p.has(&["--amend"]);
}

fn pull(rest: &[Word], out: &mut Outcome, inv: &mut GitInvocation) {
    let p = Parsed::new(
        rest,
        &[
            "-s",
            "-X",
            "-j",
            "--strategy",
            "--upload-pack",
            "--depth",
            "--deepen",
            "-o",
            "--server-option",
        ],
    );
    out.class(CommandClass::Network)
        .class(CommandClass::Write)
        .unknown_targets();
    let rebase = p.opts.iter().any(|o| {
        matches!(o.name.as_str(), "--rebase" | "-r")
            && o.value.as_ref().is_none_or(|v| v.text != "false")
    });
    inv.rewrites_history = rebase;
    inv.creates_commit = !p.has(&["--ff-only", "--no-commit"]);
    inv.refs = p
        .positionals()
        .iter()
        .skip(1)
        .map(|w| ref_name(&w.text))
        .collect();
}

fn clone(rest: &[Word], out: &mut Outcome) {
    let p = Parsed::new(
        rest,
        &[
            "-b",
            "-o",
            "-c",
            "-j",
            "-u",
            "--branch",
            "--origin",
            "--depth",
            "--reference",
            "--template",
            "--config",
            "--jobs",
            "--filter",
            "--separate-git-dir",
            "--upload-pack",
            "--server-option",
            "--shallow-since",
            "--shallow-exclude",
            "--bundle-uri",
            "--ref-format",
        ],
    );
    out.class(CommandClass::Network).class(CommandClass::Write);
    match (p.pos.get(1), p.pos.first()) {
        (Some(dir), _) => {
            out.target(dir, TargetOp::Write, true);
        }
        (None, Some(url)) if url.is_exact() => {
            let name = basename(&url.text);
            let name = name.rsplit([':', '/']).next().unwrap_or(name);
            let name = name.strip_suffix(".git").unwrap_or(name);
            if !name.is_empty() {
                out.path(name, Kind::Exact, TargetOp::Write, true);
            }
        }
        _ => {
            out.unknown_targets();
        }
    }
}

/// Strips `refs/heads/` so a branch is named the way policy globs name it.
fn ref_name(spec: &str) -> String {
    spec.strip_prefix("refs/heads/").unwrap_or(spec).to_string()
}

fn push(rest: &[Word], out: &mut Outcome, inv: &mut GitInvocation) {
    let p = Parsed::new(
        rest,
        &["-o", "--push-option", "--repo", "--receive-pack", "--exec"],
    );
    out.class(CommandClass::Network);
    let mut force = p.has(&[
        "-f",
        "--force",
        "--force-with-lease",
        "--force-if-includes",
        "--mirror",
    ]);
    let mut destructive = p.has(&["--mirror", "--prune"]);
    let delete_all = p.has(&["-d", "--delete"]);
    let skip = usize::from(!p.has(&["--all", "--mirror", "--repo"]));
    for spec in p.positionals().iter().skip(skip) {
        let mut text = spec.text.as_str();
        if let Some(stripped) = text.strip_prefix('+') {
            force = true;
            text = stripped;
        }
        let dst = match text.split_once(':') {
            Some(("", dst)) => {
                destructive = true;
                dst
            }
            Some((_, dst)) => dst,
            None => {
                if delete_all {
                    destructive = true;
                }
                text
            }
        };
        if !dst.is_empty() {
            inv.refs.push(ref_name(dst));
        }
    }
    destructive |= delete_all;
    inv.force_push = force;
    inv.rewrites_history = force || destructive;
}

/// Whether a word names a revision rather than a path (best effort).
fn is_rev(text: &str) -> bool {
    matches!(
        text,
        "HEAD" | "@" | "ORIG_HEAD" | "FETCH_HEAD" | "MERGE_HEAD" | "CHERRY_PICK_HEAD"
    ) || text.starts_with("HEAD~")
        || text.starts_with("HEAD^")
        || text.starts_with("@{")
        || text.contains("..")
        || text.starts_with("origin/")
        || text.starts_with("upstream/")
        || (text.len() >= 7
            && text.len() <= 40
            && text.bytes().all(|b| b.is_ascii_hexdigit())
            && text.bytes().any(|b| b.is_ascii_digit()))
}

fn reset(rest: &[Word], out: &mut Outcome, inv: &mut GitInvocation) {
    let p = Parsed::new(rest, &["--pathspec-from-file"]);
    out.class(CommandClass::Write);
    let hard = p.has(&["--hard", "--merge", "--keep"]);
    let rev = p.positionals().first().filter(|w| hard || is_rev(&w.text));
    if let Some(rev) = rev {
        inv.refs.push(rev.text.clone());
    }
    let moves = rev.is_some_and(|r| r.text != "HEAD" && r.text != "@");
    inv.rewrites_history = hard || moves;
    if hard {
        out.unknown_targets();
    }
}

fn checkout(rest: &[Word], out: &mut Outcome, inv: &mut GitInvocation) {
    let p = Parsed::new(
        rest,
        &[
            "-b",
            "-B",
            "--orphan",
            "--conflict",
            "--pathspec-from-file",
            "--unified",
        ],
    );
    out.class(CommandClass::Write);
    let paths = p.after_dd();
    let forced = p.has(&["-f", "--force", "-B"]);
    if !paths.is_empty() {
        // `git checkout [<rev>] -- <paths>` overwrites working-tree files.
        inv.rewrites_history = true;
        out.targets(paths.iter().copied(), TargetOp::Write, true);
        inv.refs = words_text(&p.positionals()[..p.positionals().len().min(1)]);
    } else if let Some(branch) = p.value(&["-b", "-B", "--orphan"]) {
        inv.rewrites_history = forced;
        inv.refs.push(branch.text);
        out.unknown_targets();
    } else if p
        .positionals()
        .first()
        .is_some_and(|w| w.text == "." || w.text.starts_with("./"))
    {
        inv.rewrites_history = true;
        out.targets(p.positionals().iter().copied(), TargetOp::Write, true);
    } else {
        inv.rewrites_history = forced;
        inv.refs = words_text(&p.positionals()[..p.positionals().len().min(1)]);
        out.unknown_targets();
    }
}

fn switch(rest: &[Word], out: &mut Outcome, inv: &mut GitInvocation) {
    let p = Parsed::new(
        rest,
        &[
            "-c",
            "-C",
            "--create",
            "--force-create",
            "--orphan",
            "--conflict",
        ],
    );
    out.class(CommandClass::Write).unknown_targets();
    inv.rewrites_history = p.has(&["-f", "--force", "--discard-changes", "-C", "--force-create"]);
    match p.value(&["-c", "-C", "--create", "--force-create", "--orphan"]) {
        Some(branch) => inv.refs.push(branch.text),
        None => inv.refs = words_text(&p.positionals()[..p.positionals().len().min(1)]),
    }
}

fn restore(rest: &[Word], out: &mut Outcome, inv: &mut GitInvocation) {
    let p = Parsed::new(rest, &["-s", "--source", "--pathspec-from-file"]);
    out.class(CommandClass::Write);
    let staged_only = p.has(&["-S", "--staged"]) && !p.has(&["-W", "--worktree"]);
    if !staged_only {
        inv.rewrites_history = true;
        out.targets(p.pos.iter().copied(), TargetOp::Write, true);
        if p.pos.is_empty() {
            out.unknown_targets();
        }
    }
}

fn clean(rest: &[Word], out: &mut Outcome, inv: &mut GitInvocation) {
    let p = Parsed::new(rest, &["-e", "--exclude"]);
    if p.has(&["-n", "--dry-run"]) {
        out.class(CommandClass::Read);
        return;
    }
    out.class(CommandClass::Delete);
    inv.rewrites_history = true;
    if p.pos.is_empty() {
        out.path(".", Kind::Exact, TargetOp::Delete, true);
    } else {
        out.targets(p.pos.iter().copied(), TargetOp::Delete, true);
    }
}

fn stash(rest: &[Word], out: &mut Outcome, inv: &mut GitInvocation) {
    let p = Parsed::new(rest, &["-m", "--message"]);
    match p.pos.first().map(|w| w.text.as_str()) {
        Some("list" | "show") => {
            out.class(CommandClass::Read);
        }
        Some("drop" | "clear") => {
            out.class(CommandClass::Write);
            inv.rewrites_history = true;
        }
        _ => {
            out.class(CommandClass::Write).unknown_targets();
        }
    }
}

fn branch(rest: &[Word], out: &mut Outcome, inv: &mut GitInvocation) {
    let p = Parsed::new(
        rest,
        &[
            "-m",
            "-M",
            "-c",
            "-C",
            "-u",
            "--set-upstream-to",
            "--contains",
            "--no-contains",
            "--merged",
            "--no-merged",
            "--sort",
            "--format",
            "--points-at",
            "--color",
            "--abbrev",
        ],
    );
    let force = p.has(&["-f", "--force"]);
    let delete = p.has(&["-d", "-D", "--delete"]);
    let positional = p.positionals();
    if delete {
        out.class(CommandClass::Write);
        inv.rewrites_history = p.has(&["-D"]) || force;
        inv.refs = words_text(positional);
    } else if p.has(&[
        "-m",
        "-M",
        "-c",
        "-C",
        "-u",
        "--set-upstream-to",
        "--unset-upstream",
        "--edit-description",
    ]) {
        out.class(CommandClass::Write);
        inv.refs = p
            .values(&["-m", "-M", "-c", "-C"])
            .into_iter()
            .map(|w| w.text)
            .chain(positional.iter().map(|w| w.text.clone()))
            .collect();
    } else if positional.is_empty()
        || p.has(&[
            "-l",
            "--list",
            "-a",
            "-r",
            "-v",
            "--show-current",
            "--contains",
            "--no-contains",
            "--merged",
            "--no-merged",
        ])
    {
        out.class(CommandClass::Read);
    } else {
        out.class(CommandClass::Write);
        inv.rewrites_history = force;
        inv.refs = words_text(&positional[..1]);
    }
}

fn tag(rest: &[Word], out: &mut Outcome, inv: &mut GitInvocation) {
    let p = Parsed::new(
        rest,
        &[
            "-m",
            "-F",
            "-u",
            "--message",
            "--file",
            "--local-user",
            "--sort",
            "--format",
            "--contains",
            "--no-contains",
            "--points-at",
            "--merged",
            "--no-merged",
            "--cleanup",
        ],
    );
    let positional = p.positionals();
    if p.has(&["-d", "--delete"]) {
        out.class(CommandClass::Write);
        inv.refs = words_text(positional);
    } else if p.has(&["-l", "--list", "-v", "--verify", "-n"]) || positional.is_empty() {
        out.class(CommandClass::Read);
    } else {
        out.class(CommandClass::Write);
        inv.rewrites_history = p.has(&["-f", "--force"]);
        inv.refs = words_text(&positional[..1]);
    }
}

fn config(rest: &[Word], out: &mut Outcome) {
    let p = Parsed::new(rest, &["--file", "-f", "--blob", "--type", "--default"]);
    let sets = p.has(&[
        "--unset",
        "--unset-all",
        "--add",
        "--replace-all",
        "-e",
        "--edit",
        "--rename-section",
        "--remove-section",
    ]) || (p.pos.len() >= 2
        && !p.has(&[
            "--get",
            "--get-all",
            "--get-regexp",
            "-l",
            "--list",
            "--get-urlmatch",
        ]));
    out.class(if sets {
        CommandClass::Write
    } else {
        CommandClass::Read
    });
}

fn worktree(rest: &[Word], out: &mut Outcome) {
    let p = Parsed::new(rest, &["-b", "-B", "--reason", "--expire"]);
    match p.pos.first().map(|w| w.text.as_str()) {
        Some("add") => {
            out.class(CommandClass::Write);
            if let Some(dir) = p.pos.get(1) {
                out.target(dir, TargetOp::Write, true);
            }
        }
        Some("remove") => {
            out.class(CommandClass::Delete);
            if let Some(dir) = p.pos.get(1) {
                out.target(dir, TargetOp::Delete, true);
            }
        }
        Some("prune" | "move" | "lock" | "unlock" | "repair") => {
            out.class(CommandClass::Write).unknown_targets();
        }
        _ => {
            out.class(CommandClass::Read);
        }
    }
}

/// Targets of a move: every source is deleted, the destination written. Both are marked
/// recursive because a source may be a directory and the destination may be one that the
/// sources land inside.
pub(crate) fn move_targets(pos: &[&Word], no_target_dir: bool, out: &mut Outcome) {
    let Some((dest, sources)) = pos.split_last() else {
        return;
    };
    if sources.is_empty() {
        return;
    }
    out.targets(sources.iter().copied(), TargetOp::Delete, true);
    out.target(dest, TargetOp::Write, !no_target_dir);
}
