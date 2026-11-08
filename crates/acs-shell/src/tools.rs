//! Handlers for individual programs: how each one's arguments map to targets.

use acs_core::gate::{CommandClass, TargetOp};

use crate::Dialect;
use crate::args::{Chdir, Outcome, Parsed};
use crate::git::move_targets;
use crate::table::classify;
use crate::word::{Kind, Word, basename};

/// Marks a `find` placeholder inside the inner command of `-exec`.
const FIND_PLACEHOLDER: &str = "\u{0}";

// ---------------------------------------------------------------------------------------------
// Reading
// ---------------------------------------------------------------------------------------------

/// Programs that read the files they are given; `out_opts` name options whose value is an output
/// file (`sort -o`).
pub(crate) fn read_files(args: &[Word], value_opts: &[&str], out_opts: &[&str], out: &mut Outcome) {
    let p = Parsed::new(args, value_opts);
    out.class(CommandClass::Read);
    let recursive = p.has(&["-R", "--recursive"]);
    out.targets(p.pos.iter().copied(), TargetOp::Read, recursive);
    if let Some(file) = p.value(out_opts) {
        out.class(CommandClass::Write);
        out.target(&file, TargetOp::Write, false);
    }
}

/// Whether a search program descends into directories.
#[derive(Clone, Copy)]
pub(crate) enum Recursion {
    /// Always (`rg`).
    Always,
    /// With `-r`/`-R` (`grep`).
    ByFlag,
    /// Never (`jq`, where `-r` means raw output).
    Never,
}

/// `grep`-like programs: the first positional is the pattern unless a pattern option is given.
pub(crate) fn search(
    args: &[Word],
    value_opts: &[&str],
    pattern_opts: &[&str],
    recursion: Recursion,
    out: &mut Outcome,
) {
    let p = Parsed::new(args, value_opts);
    out.class(CommandClass::Read);
    let skip = usize::from(!p.has(pattern_opts));
    let recursive = match recursion {
        Recursion::Always => true,
        Recursion::ByFlag => p.has(&["-r", "-R", "--recursive"]),
        Recursion::Never => false,
    };
    out.targets(p.pos.iter().copied().skip(skip), TargetOp::Read, recursive);
}

pub(crate) fn awk(args: &[Word], out: &mut Outcome) {
    let p = Parsed::new(
        args,
        &[
            "-F",
            "-v",
            "-f",
            "-e",
            "-i",
            "--file",
            "--source",
            "--assign",
            "--field-separator",
            "-W",
            "-M",
        ],
    );
    let has_script_opt = p.has(&["-f", "-e", "--file", "--source"]);
    let script = if has_script_opt { None } else { p.pos.first() };
    let files = p.pos.iter().copied().skip(usize::from(!has_script_opt));
    let in_place = p.value(&["-i"]).is_some_and(|v| v.text == "inplace");
    let mut risky = false;
    if let Some(script) = script {
        let text = script.text.replace("||", "");
        risky = !script.is_exact()
            || text.contains("system(")
            || text.contains('|')
            || text
                .find("print")
                .is_some_and(|at| text[at..].contains('>'));
    }
    out.class(CommandClass::Read);
    if in_place {
        out.class(CommandClass::Write);
        out.targets(files, TargetOp::Write, false);
    } else {
        out.targets(files, TargetOp::Read, false);
    }
    if risky {
        out.class(CommandClass::Write).and_unknown();
    }
}

// ---------------------------------------------------------------------------------------------
// Removing, moving, copying
// ---------------------------------------------------------------------------------------------

pub(crate) fn rm(args: &[Word], out: &mut Outcome) {
    let p = Parsed::new(args, &[]);
    out.class(CommandClass::Delete);
    let recursive = p.has(&["-r", "-R", "--recursive"]);
    out.targets(p.pos.iter().copied(), TargetOp::Delete, recursive);
}

/// `rmdir`, `unlink`, `shred`, `trash`: delete every positional.
pub(crate) fn delete_all(args: &[Word], out: &mut Outcome) {
    let p = Parsed::new(
        args,
        &["-n", "-s", "--iterations", "--size", "--random-source"],
    );
    out.class(CommandClass::Delete);
    out.targets(
        p.pos.iter().copied(),
        TargetOp::Delete,
        p.has(&["-p", "--parents"]),
    );
}

pub(crate) fn mv(args: &[Word], out: &mut Outcome) {
    let p = Parsed::new(args, &["-t", "--target-directory", "-S", "--suffix"]);
    out.class(CommandClass::Move);
    if let Some(dir) = p.value(&["-t", "--target-directory"]) {
        out.targets(p.pos.iter().copied(), TargetOp::Delete, true);
        out.target(&dir, TargetOp::Write, true);
    } else {
        move_targets(&p.pos, p.has(&["-T", "--no-target-directory"]), out);
    }
}

/// `cp` and `install`: sources are read, the destination written. The destination is marked
/// recursive because it may be an existing directory the sources land inside.
pub(crate) fn cp(args: &[Word], out: &mut Outcome) {
    let p = Parsed::new(
        args,
        &[
            "-t",
            "--target-directory",
            "-S",
            "--suffix",
            "-m",
            "--mode",
            "-o",
            "--owner",
            "-g",
            "--group",
            "--backup",
            "--preserve",
            "--sparse",
            "-Z",
            "--context",
        ],
    );
    out.class(CommandClass::Write);
    let recursive = p.has(&["-r", "-R", "-a", "--recursive", "--archive"]);
    if p.has(&["-d"]) && !p.has(&["-a"]) {
        // `install -d`: create directories.
        out.targets(p.pos.iter().copied(), TargetOp::Write, false);
        return;
    }
    if let Some(dir) = p.value(&["-t", "--target-directory"]) {
        out.targets(p.pos.iter().copied(), TargetOp::Read, recursive);
        out.target(&dir, TargetOp::Write, true);
        return;
    }
    let Some((dest, sources)) = p.pos.split_last() else {
        return;
    };
    if sources.is_empty() {
        return;
    }
    out.targets(sources.iter().copied(), TargetOp::Read, recursive);
    out.target(
        dest,
        TargetOp::Write,
        recursive || !p.has(&["-T", "--no-target-directory"]),
    );
}

pub(crate) fn ln(args: &[Word], out: &mut Outcome) {
    let p = Parsed::new(args, &["-t", "--target-directory", "-S", "--suffix"]);
    out.class(CommandClass::Write);
    if let Some(dir) = p.value(&["-t", "--target-directory"]) {
        out.target(&dir, TargetOp::Write, true);
    } else if let [only] = p.pos.as_slice() {
        // `ln TARGET` links into the current directory under the target's name.
        if only.is_exact() {
            out.path(basename(&only.text), Kind::Exact, TargetOp::Write, false);
        } else {
            out.unknown_targets();
        }
    } else if let Some(link) = p.pos.last() {
        out.target(
            link,
            TargetOp::Write,
            !p.has(&["-T", "--no-target-directory"]),
        );
    }
}

/// `touch`, `mkdir`, `tee`, `truncate`: every positional is written.
pub(crate) fn write_all(args: &[Word], value_opts: &[&str], out: &mut Outcome) {
    let p = Parsed::new(args, value_opts);
    out.class(CommandClass::Write);
    out.targets(p.pos.iter().copied(), TargetOp::Write, false);
}

pub(crate) fn mktemp(args: &[Word], out: &mut Outcome) {
    let p = Parsed::new(args, &["-p", "--tmpdir", "--suffix"]);
    if p.has(&["-u", "--dry-run"]) {
        out.class(CommandClass::Read);
        return;
    }
    out.class(CommandClass::Write);
    let dir = p.has(&["-d", "--directory"]);
    out.targets(p.pos.iter().copied(), TargetOp::Write, dir);
    if let Some(parent) = p.value(&["-p", "--tmpdir"]) {
        out.target(&parent, TargetOp::Write, true);
    }
}

/// `chmod`, `chown`, `chgrp`: the first positional is the mode or owner unless `--reference`.
pub(crate) fn chmod(args: &[Word], out: &mut Outcome) {
    let p = Parsed::new(args, &["--reference", "--from"]);
    out.class(CommandClass::Write);
    // `chmod -x file` reads like an option cluster but is a mode.
    let mode_as_option = args.iter().take_while(|w| w.is_option()).any(|w| {
        let flags = &w.text[1..];
        !flags.starts_with('-')
            && !flags.is_empty()
            && (flags.chars().all(|c| "rwxXst".contains(c))
                || flags.chars().all(|c| c.is_ascii_digit()))
    });
    let skip = usize::from(!p.has(&["--reference"]) && !mode_as_option);
    out.targets(
        p.pos.iter().copied().skip(skip),
        TargetOp::Write,
        p.has(&["-R", "--recursive"]),
    );
}

pub(crate) fn dd(args: &[Word], out: &mut Outcome) {
    out.class(CommandClass::Read);
    for arg in args {
        if let Some(path) = arg.text.strip_prefix("of=") {
            out.class(CommandClass::Write);
            out.target(
                &Word {
                    text: path.to_string(),
                    kind: arg.kind,
                },
                TargetOp::Write,
                false,
            );
        } else if let Some(path) = arg.text.strip_prefix("if=") {
            out.target(
                &Word {
                    text: path.to_string(),
                    kind: arg.kind,
                },
                TargetOp::Read,
                false,
            );
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Editing in place
// ---------------------------------------------------------------------------------------------

pub(crate) fn sed(args: &[Word], out: &mut Outcome) {
    let p = Parsed::new(
        args,
        &["-e", "-f", "-l", "--expression", "--file", "--line-length"],
    );
    let script_given = p.has(&["-e", "-f", "--expression", "--file"]);
    let in_place = p.has(&["-i", "--in-place"]);
    let files = p.pos.iter().copied().skip(usize::from(!script_given));
    if in_place {
        out.class(CommandClass::Write);
        out.targets(files, TargetOp::Write, false);
    } else {
        out.class(CommandClass::Read);
        out.targets(files, TargetOp::Read, false);
    }
    // Scripts can write files (`w out`, `s/a/b/w out`) and run commands (`e`, `s/a/b/e`).
    let scripts = p.values(&["-e", "--expression"]).into_iter().chain(
        p.pos
            .first()
            .filter(|_| !script_given)
            .map(|w| (*w).clone()),
    );
    for script in scripts {
        if sed_script_has_side_effects(&script.text) {
            out.class(CommandClass::Write).and_unknown();
        }
    }
    if p.has(&["-f", "--file"]) {
        // A script file can contain anything.
        out.class(CommandClass::Write).and_unknown();
    }
}

/// Whether a `sed` script contains a write (`w`) or execute (`e`) command or flag. Scripts are
/// split on `;`, newlines and braces, which is close enough for scripts that matter here.
fn sed_script_has_side_effects(script: &str) -> bool {
    script
        .split([';', '\n', '{', '}'])
        .any(sed_command_has_side_effects)
}

fn sed_command_has_side_effects(command: &str) -> bool {
    let mut rest = command.trim_start();
    // Skip the addresses: numbers, `$`, ranges and `/regex/` (with optional `I`/`M`).
    loop {
        rest = rest.trim_start_matches(|c: char| {
            c.is_ascii_digit() || c.is_whitespace() || matches!(c, ',' | '$' | '!' | '~' | '+')
        });
        let Some(after) = rest.strip_prefix('/') else {
            break;
        };
        let mut escaped = false;
        let mut end = None;
        for (index, c) in after.char_indices() {
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == '/' {
                end = Some(index + 1);
                break;
            }
        }
        match end {
            Some(end) => rest = after[end..].trim_start_matches(['I', 'M']),
            None => return false,
        }
    }
    match rest.chars().next() {
        Some('w' | 'W' | 'e') => true,
        Some('s') => {
            // `s<d>pattern<d>replacement<d>flags`: the flags `w` and `e` write and execute.
            let mut chars = rest[1..].chars();
            let Some(delimiter) = chars.next() else {
                return false;
            };
            let mut seen = 0;
            let mut escaped = false;
            let mut flags = String::new();
            for c in chars {
                if seen >= 2 {
                    flags.push(c);
                } else if escaped {
                    escaped = false;
                } else if c == '\\' {
                    escaped = true;
                } else if c == delimiter {
                    seen += 1;
                }
            }
            seen >= 2 && flags.contains(['w', 'e'])
        }
        _ => false,
    }
}

pub(crate) fn perl(args: &[Word], out: &mut Outcome) {
    // Perl clusters switches: `-pi -e`, `-i.bak`, `-lane`. Walk them by hand.
    let mut in_place = false;
    let mut script_given = false;
    let mut i = 0;
    while i < args.len() && args[i].is_option() && !args[i].text.starts_with("--") {
        let flags = &args[i].text[1..];
        i += 1;
        for (index, ch) in flags.char_indices() {
            match ch {
                'i' => {
                    in_place = true;
                    break;
                }
                'e' | 'E' => {
                    script_given = true;
                    if index + 1 == flags.len() {
                        i += 1;
                    }
                    break;
                }
                'I' | 'M' | 'm' | 'x' | 'F' | 'C' | 'l' | '0' | 'd' | 'D' | 'V' => break,
                _ => {}
            }
        }
    }
    if i < args.len() && args[i].text == "--" {
        i += 1;
    }
    if !in_place {
        if args
            .iter()
            .any(|a| matches!(a.text.as_str(), "-v" | "-V" | "--version"))
        {
            out.class(CommandClass::Read);
        } else {
            out.and_unknown();
        }
        return;
    }
    out.class(CommandClass::Write);
    let files = args.get(i..).unwrap_or_default();
    let files = if script_given {
        files
    } else {
        files.get(1..).unwrap_or_default()
    };
    out.targets(files.iter(), TargetOp::Write, false);
}

// ---------------------------------------------------------------------------------------------
// find and xargs
// ---------------------------------------------------------------------------------------------

/// `find` primaries that take one argument (so a following `-delete` is a value, not an action).
const FIND_VALUE_PRIMARIES: &[&str] = &[
    "-name",
    "-iname",
    "-path",
    "-ipath",
    "-wholename",
    "-iwholename",
    "-regex",
    "-iregex",
    "-regextype",
    "-type",
    "-xtype",
    "-size",
    "-mtime",
    "-atime",
    "-ctime",
    "-mmin",
    "-amin",
    "-cmin",
    "-newer",
    "-user",
    "-group",
    "-perm",
    "-maxdepth",
    "-mindepth",
    "-lname",
    "-ilname",
    "-samefile",
    "-links",
    "-inum",
    "-fstype",
    "-uid",
    "-gid",
    "-printf",
    "-used",
    "-context",
];

pub(crate) fn find(args: &[Word], out: &mut Outcome) {
    let mut i = 0;
    while i < args.len() {
        match args[i].text.as_str() {
            "-H" | "-L" | "-P" => i += 1,
            "-D" if args[i].is_exact() => i += 2,
            text if args[i].is_exact() && (text.starts_with("-O") || text.starts_with("-D")) => {
                i += 1
            }
            _ => break,
        }
    }
    let rest = args.get(i..).unwrap_or_default();
    let split = rest
        .iter()
        .position(|w| {
            w.is_exact() && (w.text.starts_with('-') || matches!(w.text.as_str(), "(" | ")" | "!"))
        })
        .unwrap_or(rest.len());
    let default_start = [Word::exact(".")];
    let starts: &[Word] = if split == 0 {
        &default_start
    } else {
        &rest[..split]
    };
    let expr = &rest[split..];

    let mut deleting = false;
    let mut acting = false;
    let mut j = 0;
    while j < expr.len() {
        let primary = &expr[j];
        j += 1;
        if !primary.is_exact() {
            continue;
        }
        let name = primary.text.as_str();
        if FIND_VALUE_PRIMARIES.contains(&name) || name.starts_with("-newer") {
            j += 1;
            continue;
        }
        match name {
            "-delete" => {
                deleting = true;
                acting = true;
            }
            "-fprint" | "-fprint0" | "-fls" | "-fprintf" => {
                if let Some(file) = expr.get(j) {
                    out.class(CommandClass::Write);
                    out.target(file, TargetOp::Write, false);
                    acting = true;
                }
                j += 1;
            }
            "-exec" | "-execdir" | "-ok" | "-okdir" => {
                let end = expr[j..]
                    .iter()
                    .position(|w| w.is_exact() && (w.text == ";" || w.text == "+"))
                    .map_or(expr.len(), |at| j + at);
                let inner: Vec<Word> = expr[j..end]
                    .iter()
                    .map(|w| Word {
                        text: w.text.replace("{}", FIND_PLACEHOLDER),
                        kind: w.kind,
                    })
                    .collect();
                j = (end + 1).min(expr.len());
                acting = true;
                if !inner.is_empty() {
                    merge_find_exec(&inner, starts, out);
                }
            }
            _ => {}
        }
    }
    if deleting {
        out.class(CommandClass::Delete);
        out.targets(starts.iter(), TargetOp::Delete, true);
    }
    if !acting {
        out.class(CommandClass::Read);
        out.targets(starts.iter(), TargetOp::Read, true);
    }
}

fn merge_find_exec(inner: &[Word], starts: &[Word], out: &mut Outcome) {
    let (_, effect) = classify(inner, false);
    for class in &effect.classes {
        out.class(*class);
    }
    for target in &effect.targets {
        if target.path.contains(FIND_PLACEHOLDER) {
            for start in starts {
                out.target(start, target.op, true);
            }
        } else {
            out.targets.push(target.clone());
        }
    }
    if effect.unknown_targets {
        out.unknown_targets();
    }
    for (dialect, script) in effect.nested {
        out.nested
            .push((dialect, script.replace(FIND_PLACEHOLDER, "$__ARG__")));
    }
}

pub(crate) fn xargs(args: &[Word], out: &mut Outcome) {
    const VALUE_OPTS: &[&str] = &[
        "-n",
        "-P",
        "-s",
        "-L",
        "-l",
        "-E",
        "-e",
        "-d",
        "-a",
        "--max-args",
        "--max-procs",
        "--max-chars",
        "--max-lines",
        "--delimiter",
        "--arg-file",
        "--eof",
    ];
    let mut replace: Option<String> = None;
    let mut i = 0;
    while i < args.len() && args[i].is_option() {
        let text = args[i].text.as_str();
        i += 1;
        if text == "--" {
            break;
        }
        if text == "-I" || text == "--replace" {
            if text == "-I" {
                replace = args.get(i).map(|w| w.text.clone());
                i += 1;
            } else {
                replace = Some("{}".into());
            }
        } else if let Some(r) = text
            .strip_prefix("-I")
            .or_else(|| text.strip_prefix("--replace="))
        {
            replace = Some(r.to_string());
        } else if text == "-i" {
            replace = Some("{}".into());
        } else if VALUE_OPTS.contains(&text) {
            i += 1;
        }
    }
    let command = args.get(i..).unwrap_or_default();
    if command.is_empty() {
        out.class(CommandClass::Read);
        return;
    }
    let mut inner: Vec<Word> = command.to_vec();
    if let Some(token) = &replace {
        for word in &mut inner {
            if word.text == *token {
                *word = Word::opaque(word.text.clone());
            }
        }
    }
    let (_, mut effect) = classify(&inner, false);
    if let Some(token) = &replace {
        for (_, script) in &mut effect.nested {
            *script = script.replace(token.as_str(), "$__ARG__");
        }
    }
    *out = effect;
    out.and_unknown();
}

// ---------------------------------------------------------------------------------------------
// Archives and compression
// ---------------------------------------------------------------------------------------------

pub(crate) fn tar(args: &[Word], out: &mut Outcome) {
    let mut words: Vec<Word> = args.to_vec();
    // Old style: `tar xzf archive` without a dash.
    if let Some(first) = words.first_mut()
        && first.is_exact()
        && !first.text.starts_with('-')
        && first.text.chars().all(|c| c.is_ascii_alphabetic())
        && first.text.chars().any(|c| "xtcruAd".contains(c))
    {
        first.text.insert(0, '-');
    }
    let p = Parsed::new(
        &words,
        &[
            "-f",
            "-C",
            "-I",
            "-T",
            "-X",
            "-b",
            "-L",
            "-H",
            "-N",
            "-V",
            "--file",
            "--directory",
            "--use-compress-program",
            "--files-from",
            "--exclude-from",
            "--exclude",
            "--transform",
            "--strip-components",
            "--newer",
            "--newer-mtime",
            "--to-command",
            "--owner",
            "--group",
            "--mode",
            "--format",
            "--index-file",
            "--one-top-level",
        ],
    );
    let archive = p.value(&["-f", "--file"]).filter(|w| w.text != "-");
    let dir = p.value(&["-C", "--directory"]);
    let extract = p.has(&["-x", "--extract", "--get"]);
    let create = p.has(&[
        "-c", "--create", "-r", "--append", "-u", "--update", "--delete",
    ]);
    if extract && !p.has(&["-O", "--to-stdout"]) {
        // Extraction can overwrite anything below the target directory.
        out.class(CommandClass::Write);
        if let Some(archive) = &archive {
            out.target(archive, TargetOp::Read, false);
        }
        match &dir {
            Some(dir) => out.target(dir, TargetOp::Write, true),
            None => out.path(".", Kind::Exact, TargetOp::Write, true),
        };
    } else if create {
        out.class(CommandClass::Write);
        if let Some(archive) = &archive {
            out.target(archive, TargetOp::Write, false);
        }
        // Members are read relative to `-C dir`.
        let members = out.targets.len();
        out.targets(p.pos.iter().copied(), TargetOp::Read, true);
        if let Some(dir) = &dir {
            out.rebase_from(members, dir);
        }
    } else {
        out.class(CommandClass::Read);
        if let Some(archive) = &archive {
            out.target(archive, TargetOp::Read, false);
        }
    }
}

pub(crate) fn unzip(args: &[Word], out: &mut Outcome) {
    let p = Parsed::new(args, &["-d", "-x", "-P", "-O", "-I", "--password"]);
    if p.has(&["-l", "-t", "-p", "-Z", "-v"]) {
        out.class(CommandClass::Read);
        return;
    }
    out.class(CommandClass::Write);
    if let Some(archive) = p.pos.first() {
        out.target(archive, TargetOp::Read, false);
    }
    match p.value(&["-d"]) {
        Some(dir) => out.target(&dir, TargetOp::Write, true),
        None => out.path(".", Kind::Exact, TargetOp::Write, true),
    };
}

pub(crate) fn zip(args: &[Word], out: &mut Outcome) {
    let p = Parsed::new(
        args,
        &[
            "-b", "-n", "-t", "-x", "-i", "-P", "-O", "-tt", "-ds", "-dc", "-fz",
        ],
    );
    out.class(CommandClass::Write);
    let mut iter = p.pos.iter().copied();
    if let Some(archive) = iter.next() {
        out.target(archive, TargetOp::Write, false);
    }
    out.targets(iter, TargetOp::Read, p.has(&["-r"]));
}

/// `gzip`, `xz`, ...: replace each file by its compressed (or decompressed) form unless the
/// output goes to stdout.
pub(crate) fn compress(args: &[Word], out: &mut Outcome) {
    let p = Parsed::new(args, &["-S", "--suffix", "-T", "--threads"]);
    if p.has(&[
        "-c",
        "--stdout",
        "--to-stdout",
        "-t",
        "--test",
        "-l",
        "--list",
    ]) {
        out.class(CommandClass::Read);
        out.targets(p.pos.iter().copied(), TargetOp::Read, false);
    } else {
        out.class(CommandClass::Write);
        out.targets(
            p.pos.iter().copied(),
            TargetOp::Write,
            p.has(&["-r", "--recursive"]),
        );
    }
}

// ---------------------------------------------------------------------------------------------
// Network
// ---------------------------------------------------------------------------------------------

/// The file name a download of `url` is saved under.
fn url_file_name(url: &str) -> String {
    let without_scheme = url.split_once("://").map_or(url, |(_, rest)| rest);
    let path = without_scheme.split(['?', '#']).next().unwrap_or("");
    let name = match path.split_once('/') {
        Some((_, rest)) => basename(rest),
        None => "",
    };
    if name.is_empty() {
        "index.html".into()
    } else {
        name.to_string()
    }
}

pub(crate) fn curl(args: &[Word], out: &mut Outcome) {
    let p = Parsed::new(
        args,
        &[
            "-o",
            "-H",
            "-d",
            "-X",
            "-u",
            "-A",
            "-e",
            "-F",
            "-T",
            "-K",
            "-b",
            "-c",
            "-m",
            "-w",
            "-x",
            "-y",
            "-Y",
            "-z",
            "-r",
            "-C",
            "-D",
            "-E",
            "-Q",
            "-U",
            "-P",
            "--output",
            "--header",
            "--data",
            "--data-raw",
            "--data-binary",
            "--data-urlencode",
            "--data-ascii",
            "--request",
            "--user",
            "--user-agent",
            "--referer",
            "--form",
            "--upload-file",
            "--config",
            "--cookie",
            "--cookie-jar",
            "--max-time",
            "--write-out",
            "--proxy",
            "--retry",
            "--connect-timeout",
            "--cacert",
            "--cert",
            "--key",
            "--resolve",
            "--url-query",
            "--json",
            "--output-dir",
            "--dump-header",
            "--limit-rate",
            "--range",
            "--continue-at",
            "--max-redirs",
            "--url",
            "--retry-delay",
            "--retry-max-time",
            "--proxy-user",
            "--oauth2-bearer",
            "--aws-sigv4",
            "--interface",
            "--local-port",
            "--keepalive-time",
            "--speed-limit",
            "--speed-time",
        ],
    );
    out.class(CommandClass::Network);
    let base = p.value(&["--output-dir"]);
    let start = out.targets.len();
    for file in p.values(&[
        "-o",
        "--output",
        "-c",
        "--cookie-jar",
        "-D",
        "--dump-header",
    ]) {
        if file.text != "-" {
            out.class(CommandClass::Write);
            out.target(&file, TargetOp::Write, false);
        }
    }
    if p.has(&["-O", "--remote-name", "--remote-name-all"]) {
        out.class(CommandClass::Write);
        let mut urls: Vec<Word> = p.pos.iter().map(|w| (*w).clone()).collect();
        urls.extend(p.values(&["--url"]));
        for url in &urls {
            if url.is_exact() {
                out.path(
                    &url_file_name(&url.text),
                    Kind::Exact,
                    TargetOp::Write,
                    false,
                );
            } else {
                out.unknown_targets();
            }
        }
    }
    for file in p.values(&["-T", "--upload-file", "-K", "--config"]) {
        out.target(&file, TargetOp::Read, false);
    }
    for data in p.values(&[
        "-d",
        "--data",
        "--data-binary",
        "--data-raw",
        "--data-urlencode",
        "--data-ascii",
        "--json",
        "-F",
        "--form",
    ]) {
        let file = data
            .text
            .split_once('=')
            .map_or(data.text.as_str(), |(_, v)| v);
        if let Some(path) = file
            .strip_prefix('@')
            .or_else(|| data.text.strip_prefix('@'))
        {
            out.target(
                &Word {
                    text: path.to_string(),
                    kind: data.kind,
                },
                TargetOp::Read,
                false,
            );
        }
    }
    if let Some(dir) = base {
        out.rebase_from(start, &dir);
    }
}

pub(crate) fn wget(args: &[Word], out: &mut Outcome) {
    let p = Parsed::new(
        args,
        &[
            "-O",
            "-P",
            "-o",
            "-a",
            "-i",
            "-e",
            "-U",
            "-t",
            "-T",
            "-w",
            "-Q",
            "-l",
            "-A",
            "-R",
            "-D",
            "-I",
            "-X",
            "-B",
            "--output-document",
            "--directory-prefix",
            "--output-file",
            "--append-output",
            "--input-file",
            "--execute",
            "--user-agent",
            "--tries",
            "--timeout",
            "--wait",
            "--quota",
            "--level",
            "--accept",
            "--reject",
            "--domains",
            "--include-directories",
            "--exclude-directories",
            "--base",
            "--header",
            "--user",
            "--password",
            "--post-data",
            "--post-file",
            "--referer",
            "--load-cookies",
            "--save-cookies",
            "--ca-certificate",
            "--certificate",
            "--private-key",
            "--limit-rate",
            "--bind-address",
            "--http-user",
            "--http-password",
            "--proxy-user",
            "--proxy-password",
        ],
    );
    out.class(CommandClass::Network);
    let prefix = p.value(&["-P", "--directory-prefix"]);
    let start = out.targets.len();
    if let Some(file) = p.value(&["-O", "--output-document"]) {
        if file.text != "-" {
            out.class(CommandClass::Write);
            out.target(&file, TargetOp::Write, false);
        }
    } else if p.has(&[
        "-r",
        "--recursive",
        "-m",
        "--mirror",
        "-p",
        "--page-requisites",
    ]) {
        out.class(CommandClass::Write);
        out.path(".", Kind::Exact, TargetOp::Write, true);
    } else {
        out.class(CommandClass::Write);
        for url in p.pos.iter().copied() {
            if url.is_exact() {
                out.path(
                    &url_file_name(&url.text),
                    Kind::Exact,
                    TargetOp::Write,
                    false,
                );
            } else {
                out.unknown_targets();
            }
        }
        if p.has(&["-i", "--input-file"]) {
            out.unknown_targets();
        }
    }
    if let Some(prefix) = prefix {
        out.rebase_from(start, &prefix);
        if start == out.targets.len() {
            out.target(&prefix, TargetOp::Write, true);
        }
    }
    for log in p.values(&["-o", "--output-file", "-a", "--append-output"]) {
        out.target(&log, TargetOp::Write, false);
    }
    if let Some(list) = p.value(&["-i", "--input-file"]) {
        out.target(&list, TargetOp::Read, false);
    }
}

/// Whether an `rsync`/`scp` operand names a remote location (`host:path`, `user@host:path`).
fn is_remote(word: &Word) -> bool {
    let text = word.text.as_str();
    if text.starts_with("rsync://") || text.starts_with("ssh://") {
        return true;
    }
    match text.find(':') {
        Some(colon) => {
            let before = &text[..colon];
            !before.contains('/')
                && !(before.len() == 1 && before.as_bytes()[0].is_ascii_alphabetic())
        }
        None => false,
    }
}

pub(crate) fn rsync(args: &[Word], out: &mut Outcome) {
    let p = Parsed::new(
        args,
        &[
            "-e",
            "-f",
            "-B",
            "-M",
            "-T",
            "-@",
            "--rsh",
            "--filter",
            "--exclude",
            "--include",
            "--exclude-from",
            "--include-from",
            "--files-from",
            "--port",
            "--bwlimit",
            "--backup-dir",
            "--link-dest",
            "--compare-dest",
            "--log-file",
            "--partial-dir",
            "--timeout",
            "--contimeout",
            "--block-size",
            "--rsync-path",
            "--temp-dir",
            "--chmod",
            "--chown",
            "--usermap",
            "--groupmap",
            "--remote-option",
            "--max-size",
            "--min-size",
            "--info",
            "--debug",
            "--out-format",
            "--suffix",
            "--password-file",
            "--sockopts",
        ],
    );
    let dry = p.has(&["-n", "--dry-run"]);
    let recursive = p.has(&["-r", "-a", "--recursive", "--archive"]);
    let remote = p.pos.iter().any(|w| is_remote(w));
    if remote {
        out.class(CommandClass::Network);
    }
    if dry {
        out.class(CommandClass::Read);
        return;
    }
    let Some((dest, sources)) = p.pos.split_last() else {
        out.class(CommandClass::Read);
        return;
    };
    let delete = p.opts.iter().any(|o| o.name.starts_with("--delete"));
    if !is_remote(dest) {
        out.class(CommandClass::Write);
        out.target(dest, TargetOp::Write, true);
        if delete {
            out.class(CommandClass::Delete);
            out.target(dest, TargetOp::Delete, true);
        }
    }
    for source in sources {
        if is_remote(source) {
            continue;
        }
        out.target(source, TargetOp::Read, recursive);
        if p.has(&["--remove-source-files"]) {
            out.class(CommandClass::Delete);
            out.target(source, TargetOp::Delete, recursive);
        }
    }
    if p.has(&["--files-from"]) {
        out.unknown_targets();
    }
}

pub(crate) fn scp(args: &[Word], out: &mut Outcome) {
    let p = Parsed::new(
        args,
        &["-P", "-i", "-F", "-o", "-l", "-S", "-c", "-J", "-D"],
    );
    out.class(CommandClass::Network);
    let recursive = p.has(&["-r"]);
    let Some((dest, sources)) = p.pos.split_last() else {
        return;
    };
    if !is_remote(dest) {
        out.class(CommandClass::Write);
        out.target(dest, TargetOp::Write, true);
    }
    for source in sources {
        if !is_remote(source) {
            out.target(source, TargetOp::Read, recursive);
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Interpreters, shells and wrappers of other commands
// ---------------------------------------------------------------------------------------------

/// `sh`, `bash`, `zsh`...: `-c script` is analyzed as a nested script; the command itself stays
/// unknown, since the shell may do more than the script text shows.
pub(crate) fn shell(args: &[Word], out: &mut Outcome) {
    out.and_unknown();
    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];
        i += 1;
        if !arg.is_option() {
            return;
        }
        if arg.text.starts_with("--") {
            if matches!(arg.text.as_str(), "--rcfile" | "--init-file") {
                i += 1;
            }
            continue;
        }
        let flags = &arg.text[1..];
        if flags.contains('c') {
            if let Some(script) = args.get(i).filter(|w| nestable(w)) {
                out.nested.push((Dialect::Bash, script.text.clone()));
            }
            return;
        }
        if flags.contains('o') || flags.contains('O') {
            i += 1;
        }
    }
}

/// `powershell`/`pwsh`: `-Command` text is analyzed as PowerShell.
pub(crate) fn powershell(args: &[Word], out: &mut Outcome) {
    out.and_unknown();
    let mut i = 0;
    while i < args.len() {
        let lower = args[i].text.to_ascii_lowercase();
        i += 1;
        let Some(name) = lower.strip_prefix('-').or_else(|| lower.strip_prefix('/')) else {
            return;
        };
        if !name.is_empty() && "command".starts_with(name) {
            let script: Vec<&str> = args[i..].iter().map(|w| w.text.as_str()).collect();
            if args[i..].iter().all(Word::is_exact) && !script.is_empty() {
                out.nested.push((Dialect::PowerShell, script.join(" ")));
            }
            return;
        }
        if !name.is_empty() && "encodedcommand".starts_with(name) {
            if let Some(script) = args
                .get(i)
                .filter(|w| w.is_exact())
                .and_then(|w| crate::feed::powershell_encoded(&w.text))
            {
                out.nested.push((Dialect::PowerShell, script));
            }
            return;
        }
        if !name.is_empty() && "file".starts_with(name) {
            return;
        }
        if matches!(
            name,
            "executionpolicy"
                | "ep"
                | "workingdirectory"
                | "wd"
                | "configurationname"
                | "inputformat"
                | "outputformat"
        ) {
            i += 1;
        }
    }
}

/// `cmd /c ...`: the rest of the line is analyzed as a cmd command line.
pub(crate) fn cmd_exe(args: &[Word], out: &mut Outcome) {
    out.and_unknown();
    for (index, arg) in args.iter().enumerate() {
        let lower = arg.text.to_ascii_lowercase();
        if matches!(lower.as_str(), "/c" | "/k" | "/r") {
            let script: Vec<&str> = args[index + 1..].iter().map(|w| w.text.as_str()).collect();
            if args[index + 1..].iter().all(Word::is_exact) && !script.is_empty() {
                out.nested.push((Dialect::Cmd, script.join(" ")));
            }
            return;
        }
    }
}

/// `alias name='script'`: the script runs when the alias is used, later in the same line or
/// session, so it is analyzed now.
pub(crate) fn alias(args: &[Word], out: &mut Outcome) {
    out.class(CommandClass::Read);
    for arg in args.iter().filter(|w| nestable(w)) {
        if let Some((_, script)) = arg.text.split_once('=')
            && !script.is_empty()
        {
            out.nested.push((Dialect::Bash, script.to_owned()));
        }
    }
}

/// `script -c 'cmd' [file]`: runs `cmd` in a pseudo-terminal (and writes a typescript file).
/// Without `-c` it starts an interactive shell, which is unknown.
pub(crate) fn script(args: &[Word], out: &mut Outcome) {
    out.and_unknown();
    nested_option(args, &["-c", "--command"], out);
}

/// `su [user] -c 'cmd'`, `runuser -c 'cmd'`.
pub(crate) fn su(args: &[Word], out: &mut Outcome) {
    out.and_unknown();
    nested_option(args, &["-c", "--command"], out);
}

/// Pushes the value of the first of `names` (as `-c value`, `--command value` or
/// `--command=value`) as a nested bash script.
fn nested_option(args: &[Word], names: &[&str], out: &mut Outcome) {
    for (index, arg) in args.iter().enumerate() {
        if !arg.is_exact() {
            continue;
        }
        // A cluster of short flags ending in the option's letter (`-qc`) takes the next word too.
        let cluster = names.iter().any(|name| {
            name.len() == 2
                && arg.text.len() > 2
                && !arg.text.starts_with("--")
                && arg.text.starts_with('-')
                && arg.text.ends_with(&name[1..])
        });
        let value = if cluster || names.contains(&arg.text.as_str()) {
            args.get(index + 1).cloned()
        } else {
            names.iter().find_map(|name| {
                arg.text
                    .strip_prefix(&format!("{name}="))
                    .map(|v| Word::exact(v.to_owned()))
            })
        };
        if let Some(script) = value.filter(nestable) {
            out.nested.push((Dialect::Bash, script.text));
            return;
        }
    }
}

pub(crate) fn eval(args: &[Word], out: &mut Outcome) {
    out.and_unknown();
    if args.is_empty() {
        out.classes = vec![CommandClass::Read];
        out.unknown_targets = false;
        return;
    }
    if args.iter().all(nestable) {
        let script: Vec<&str> = args.iter().map(|w| w.text.as_str()).collect();
        out.nested.push((Dialect::Bash, script.join(" ")));
    }
}

/// Whether the text of a word is worth analyzing as a script: it is known, or only has
/// parameters in it. Command substitutions are analyzed where they are written, and a lone
/// parameter (`"$CMD"`) says nothing.
fn nestable(word: &Word) -> bool {
    match word.kind {
        Kind::Exact | Kind::Pattern => true,
        Kind::Partial => {
            !word.text.contains("$(")
                && !word.text.contains('`')
                && !(word.text.starts_with('$') && !word.text.contains(char::is_whitespace))
        }
        Kind::Opaque => false,
    }
}

/// `python`, `node`, ...: scripts are opaque, except a few well-known module invocations.
pub(crate) fn interpreter(args: &[Word], out: &mut Outcome) {
    if args
        .iter()
        .any(|a| matches!(a.text.as_str(), "--version" | "-V" | "--help"))
        && args.len() == 1
    {
        out.class(CommandClass::Read);
        return;
    }
    if let Some(at) = args.iter().position(|a| a.text == "-m") {
        match args.get(at + 1).map(|w| w.text.as_str()) {
            Some("pip") => {
                crate::table::run_sub(&crate::table::PIP, &args[at + 2..], out);
                return;
            }
            Some("venv" | "virtualenv") => {
                out.class(CommandClass::Write);
                let p = Parsed::new(&args[at + 2..], &["--prompt"]);
                out.targets(p.pos.iter().copied(), TargetOp::Write, true);
                return;
            }
            _ => {}
        }
    }
    out.and_unknown();
}

pub(crate) fn cd(args: &[Word], out: &mut Outcome) {
    out.class(CommandClass::Read);
    let p = Parsed::new(args, &[]);
    out.chdir = Some(Chdir::To(p.pos.first().map(|w| (*w).clone())));
}

/// Compilers: `-o file` is the output; without it the output name is not known.
pub(crate) fn compiler(args: &[Word], out: &mut Outcome) {
    let p = Parsed::new(
        args,
        &[
            "-o",
            "-I",
            "-L",
            "-l",
            "-D",
            "-U",
            "-include",
            "-isystem",
            "-std",
            "-x",
            "-MF",
            "-MT",
            "-MQ",
            "-J",
            "--out-dir",
            "--outDir",
            "-p",
            "--project",
            "-d",
            "--target",
            "-O",
            "-march",
            "-B",
            "-iquote",
            "-idirafter",
            "--sysroot",
            "-cp",
            "-classpath",
            "--class-path",
        ],
    );
    if p.has(&[
        "-E",
        "-fsyntax-only",
        "--noEmit",
        "--version",
        "-v",
        "--help",
    ]) {
        out.class(CommandClass::Read);
        return;
    }
    out.class(CommandClass::Write);
    match p.value(&["-o", "--out-dir", "--outDir", "-d"]) {
        Some(output) => {
            let dir = p.has(&["--out-dir", "--outDir", "-d"]);
            out.target(&output, TargetOp::Write, dir);
        }
        None => {
            out.unknown_targets();
        }
    }
}

/// Formatters and fixers. They rewrite their arguments unless a read-only flag is given.
pub(crate) fn format(args: &[Word], write_if: &[&str], read_if: &[&str], out: &mut Outcome) {
    let p = Parsed::new(
        args,
        &[
            "--config",
            "-c",
            "--line-length",
            "-l",
            "--select",
            "--ignore",
            "--target-version",
            "--edition",
            "--config-path",
        ],
    );
    if p.has(read_if) || !(write_if.is_empty() || p.has(write_if)) {
        out.class(CommandClass::Read);
        return;
    }
    out.class(CommandClass::Write);
    if p.pos.is_empty() {
        out.unknown_targets();
    }
    // These tools expand globs themselves, even quoted ones.
    out.pathspecs(p.pos.iter().copied(), TargetOp::Write, true);
}
