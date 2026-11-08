//! The program table: which handler understands which program, wrapper commands, and the
//! subcommand tables for package managers and similar tools. To teach the analysis a new
//! program, add its name to the matching arm of [`dispatch`] (or a new [`Sub`] table).

use acs_core::gate::{CommandClass, TargetOp};

use crate::args::{Outcome, Parsed};
use crate::git;
use crate::tools::{self, Recursion};
use crate::word::{Kind, Word};

// ---------------------------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------------------------

/// Classifies one command given as words. Leading `NAME=value` assignments and wrapper commands
/// (`sudo`, `env`, `nice`, `time`, `command`, `builtin`, `exec`, `nohup`, `timeout`...) are
/// skipped; the returned index is where the effective command starts in `words`.
/// `windows` makes program names case-insensitive and ignores their directory.
pub(crate) fn classify(words: &[Word], windows: bool) -> (usize, Outcome) {
    let first = skip_assignments(words, 0);
    let mut i = first;
    loop {
        let Some(program) = words.get(i) else {
            // Only wrappers: `sudo`, `time`, `FOO=1`.
            return (first.min(words.len().saturating_sub(1)), Outcome::read());
        };
        if !program.is_exact() {
            return (i, Outcome::unknown());
        }
        let Some((display, name)) = program_name(&program.text, windows) else {
            // `./script.sh`, `../bin/tool`, `/opt/x/tool`: runs code we cannot see.
            return (i, Outcome::unknown());
        };
        let args = &words[i + 1..];
        if let Some(wrapper) = wrapper(&name) {
            let mut used = skip_options(args, wrapper.value_opts);
            if name == "command" && args.iter().any(|a| matches!(a.text.as_str(), "-v" | "-V")) {
                return (i, Outcome::read());
            }
            if name == "env"
                && args
                    .iter()
                    .any(|a| matches!(a.text.as_str(), "-S" | "--split-string"))
            {
                return (i, Outcome::unknown());
            }
            used += wrapper.operands.min(args.len().saturating_sub(used));
            let next = skip_assignments(words, i + 1 + used);
            if next >= words.len() {
                let shell = matches!(name.as_str(), "sudo" | "doas")
                    && args
                        .iter()
                        .any(|a| matches!(a.text.as_str(), "-i" | "-s" | "--login" | "--shell"));
                return (
                    i,
                    if shell {
                        Outcome::unknown()
                    } else {
                        Outcome::read()
                    },
                );
            }
            i = next;
            continue;
        }
        let mut outcome = dispatch(&name, args);
        if display != program.text && !outcome.generic {
            outcome.program = Some(display);
        }
        return (i, outcome);
    }
}

fn skip_assignments(words: &[Word], mut i: usize) -> usize {
    while let Some(word) = words.get(i) {
        let Some((name, _)) = word.text.split_once('=') else {
            break;
        };
        let valid = !name.is_empty()
            && !name.starts_with(|c: char| c.is_ascii_digit())
            && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
        if !valid {
            break;
        }
        i += 1;
    }
    i
}

/// The directories whose programs are system tools (`/usr/bin/rm` is `rm`).
const SYSTEM_BIN_DIRS: &[&str] = &[
    "/bin",
    "/sbin",
    "/usr/bin",
    "/usr/sbin",
    "/usr/local/bin",
    "/usr/local/sbin",
    "/opt/homebrew/bin",
];

/// The program a word names, as `(display, key)`: the display name is the word without its
/// directory (and without `.exe` and case on Windows), the key additionally folds versioned
/// names (`python3.12`) into their family. `None` when the word is a path to something that is
/// not a known system tool.
fn program_name(text: &str, windows: bool) -> Option<(String, String)> {
    let separators: &[char] = if windows { &['/', '\\'] } else { &['/'] };
    let (dir, base) = match text.rfind(separators) {
        Some(at) => (Some(&text[..at]), &text[at + 1..]),
        None => (None, text),
    };
    if let Some(dir) = dir
        && !windows
        && !SYSTEM_BIN_DIRS.contains(&dir)
    {
        return None;
    }
    let mut display = base.to_string();
    if windows {
        display = display.to_ascii_lowercase();
        if let Some(stem) = display.strip_suffix(".exe") {
            display = stem.to_string();
        }
    }
    // `python3.12`, `pip3` and friends.
    for family in ["python", "pip"] {
        if let Some(version) = display.strip_prefix(family)
            && !version.is_empty()
            && version.chars().all(|c| c.is_ascii_digit() || c == '.')
        {
            return Some((display.clone(), family.to_string()));
        }
    }
    let key = display.clone();
    Some((display, key))
}

// ---------------------------------------------------------------------------------------------
// Wrappers
// ---------------------------------------------------------------------------------------------

struct Wrapper {
    /// Options that take a value.
    value_opts: &'static [&'static str],
    /// Positional operands before the wrapped command (`timeout 5 cmd`).
    operands: usize,
}

fn wrapper(name: &str) -> Option<Wrapper> {
    let (value_opts, operands): (&'static [&'static str], usize) = match name {
        "sudo" => (
            &[
                "-u",
                "-g",
                "-h",
                "-p",
                "-C",
                "-D",
                "-R",
                "-T",
                "-U",
                "-r",
                "-t",
                "--user",
                "--group",
                "--host",
                "--prompt",
                "--close-from",
                "--chdir",
                "--chroot",
                "--command-timeout",
                "--other-user",
                "--role",
                "--type",
            ],
            0,
        ),
        "doas" => (&["-u", "-C"], 0),
        "env" => (&["-u", "-C", "--unset", "--chdir"], 0),
        "nice" => (&["-n", "--adjustment"], 0),
        "ionice" => (&["-c", "-n", "-p", "-P", "-u", "--class", "--classdata"], 0),
        "time" => (&["-f", "-o", "--format", "--output"], 0),
        "timeout" => (&["-s", "-k", "--signal", "--kill-after"], 1),
        "exec" => (&["-a"], 0),
        "stdbuf" => (&["-i", "-o", "-e", "--input", "--output", "--error"], 0),
        "watch" => (&["-n", "--interval", "-w"], 0),
        "command" | "builtin" | "nohup" | "setsid" => (&[], 0),
        _ => return None,
    };
    Some(Wrapper {
        value_opts,
        operands,
    })
}

/// Number of leading words that are options of a wrapper (including their values).
fn skip_options(args: &[Word], value_opts: &[&str]) -> usize {
    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];
        if arg.text == "--" {
            return i + 1;
        }
        if !arg.is_option() {
            break;
        }
        i += 1;
        if arg.text.starts_with("--") {
            if !arg.text.contains('=') && value_opts.contains(&arg.text.as_str()) {
                i += 1;
            }
            continue;
        }
        for (index, ch) in arg.text.char_indices().skip(1) {
            if value_opts.contains(&format!("-{ch}").as_str()) {
                if index + ch.len_utf8() == arg.text.len() {
                    i += 1;
                }
                break;
            }
        }
    }
    i.min(args.len())
}

// ---------------------------------------------------------------------------------------------
// Programs
// ---------------------------------------------------------------------------------------------

/// Programs that read nothing from the file system worth listing (or do nothing at all).
const INERT: &[&str] = &[
    "echo",
    "printf",
    "pwd",
    "true",
    "false",
    ":",
    "test",
    "[",
    "date",
    "whoami",
    "id",
    "uname",
    "hostname",
    "printenv",
    "which",
    "whereis",
    "type",
    "sleep",
    "seq",
    "yes",
    "basename",
    "dirname",
    "nproc",
    "uptime",
    "ps",
    "top",
    "htop",
    "df",
    "free",
    "clear",
    "export",
    "unset",
    "set",
    "unalias",
    "read",
    "wait",
    "kill",
    "pkill",
    "killall",
    "jobs",
    "history",
    "exit",
    "return",
    "break",
    "continue",
    "getopts",
    "shift",
    "local",
    "declare",
    "typeset",
    "readonly",
    "umask",
    "ulimit",
    "let",
    "hash",
    "help",
    "man",
    "info",
    "tput",
    "stty",
    "tty",
    "groups",
    "logname",
    "locale",
    "arch",
    "cal",
    "expr",
    "bc",
    "factor",
    "tr",
    "lsb_release",
    "sw_vers",
    "pbcopy",
    "pbpaste",
    "clip",
    "wl-copy",
    "xclip",
    "disown",
    "fg",
    "bg",
    "times",
    "compgen",
    "complete",
    "shopt",
    "caller",
    "fc",
    "lsof",
    "pgrep",
    "reset",
    "sync",
    "env",
    "pidof",
    "w",
    "who",
    "last",
    "lscpu",
    "lsblk",
    "ip",
    "ifconfig",
    "netstat",
    "ss",
    "getconf",
    "ldd",
    "nm",
    "objdump",
    "size",
    "readelf",
];

const GREP_VALUE_OPTS: &[&str] = &[
    "-e",
    "-f",
    "-m",
    "-A",
    "-B",
    "-C",
    "-d",
    "-D",
    "--include",
    "--exclude",
    "--exclude-dir",
    "--regexp",
    "--file",
    "--max-count",
    "--label",
    "--color",
    "--context",
    "--after-context",
    "--before-context",
    "--binary-files",
    "--devices",
    "--directories",
    "--exclude-from",
    "--group-separator",
];

const RG_VALUE_OPTS: &[&str] = &[
    "-e",
    "-f",
    "-g",
    "-t",
    "-T",
    "-m",
    "-A",
    "-B",
    "-C",
    "-j",
    "-M",
    "-E",
    "-r",
    "--regexp",
    "--file",
    "--glob",
    "--iglob",
    "--type",
    "--type-not",
    "--type-add",
    "--max-count",
    "--context",
    "--after-context",
    "--before-context",
    "--threads",
    "--max-columns",
    "--encoding",
    "--replace",
    "--max-depth",
    "--max-filesize",
    "--sort",
    "--sortr",
    "--colors",
    "--pre",
    "--ignore-file",
    "--path-separator",
];

/// Maps a program (already stripped of wrappers) to its effect.
fn dispatch(name: &str, args: &[Word]) -> Outcome {
    let mut out = Outcome::default();
    let o = &mut out;
    match name {
        // Reading.
        _ if INERT.contains(&name) => {
            o.class(CommandClass::Read);
        }
        "cat" | "tac" | "nl" | "md5sum" | "sha1sum" | "sha224sum" | "sha256sum" | "sha384sum"
        | "sha512sum" | "cksum" | "b2sum" | "comm" | "paste" | "join" | "expand" | "unexpand"
        | "readlink" | "realpath" | "zcat" | "gzcat" | "bzcat" | "xzcat" | "diff3" | "rev"
        | "shuf" | "tsort" => tools::read_files(args, &[], &[], o),
        "head" | "tail" => tools::read_files(
            args,
            &["-n", "-c", "-s", "--lines", "--bytes", "--pid"],
            &[],
            o,
        ),
        "less" | "more" | "most" | "bat" | "batcat" => tools::read_files(
            args,
            &[
                "-p",
                "-l",
                "--pattern",
                "--language",
                "--theme",
                "--style",
                "--color",
                "--paging",
                "-P",
            ],
            &[],
            o,
        ),
        "wc" => tools::read_files(args, &["--files0-from"], &[], o),
        "od" | "xxd" | "hexdump" | "strings" | "file" | "stat" | "uniq" => tools::read_files(
            args,
            &[
                "-n", "-s", "-l", "-c", "-t", "-j", "-N", "-m", "--format", "-f", "-w",
            ],
            &[],
            o,
        ),
        "ls" | "dir" | "vdir" | "exa" | "eza" | "lsd" => tools::read_files(
            args,
            &[
                "-I",
                "-w",
                "-T",
                "--ignore",
                "--width",
                "--tabsize",
                "--color",
                "--sort",
                "--time",
                "--format",
                "--time-style",
                "--block-size",
                "--indicator-style",
                "--quoting-style",
            ],
            &[],
            o,
        ),
        "tree" => tools::read_files(
            args,
            &["-L", "-I", "-P", "--charset", "--filelimit"],
            &["-o"],
            o,
        ),
        "du" => tools::read_files(
            args,
            &[
                "-d",
                "--max-depth",
                "-B",
                "--block-size",
                "-t",
                "--threshold",
                "--exclude",
                "-X",
                "--time",
            ],
            &[],
            o,
        ),
        "diff" | "colordiff" | "delta" => tools::read_files(
            args,
            &[
                "-U",
                "-C",
                "-I",
                "-x",
                "-X",
                "--unified",
                "--context",
                "--ignore-matching-lines",
                "--exclude",
                "--from-file",
                "--to-file",
                "--color",
                "-W",
                "--width",
                "-F",
                "--label",
                "--tabsize",
            ],
            &[],
            o,
        ),
        "cmp" => tools::read_files(args, &["-n", "-i", "--bytes", "--ignore-initial"], &[], o),
        "sort" => tools::read_files(
            args,
            &[
                "-k",
                "-t",
                "-T",
                "-S",
                "--key",
                "--field-separator",
                "--buffer-size",
                "--temporary-directory",
                "--parallel",
                "--files0-from",
                "--random-source",
                "--compress-program",
                "-o",
                "--output",
            ],
            &["-o", "--output"],
            o,
        ),
        "cut" => tools::read_files(
            args,
            &[
                "-b",
                "-c",
                "-d",
                "-f",
                "--bytes",
                "--characters",
                "--delimiter",
                "--fields",
                "--output-delimiter",
            ],
            &[],
            o,
        ),
        "fold" | "fmt" => tools::read_files(args, &["-w", "--width"], &[], o),
        "column" => tools::read_files(args, &["-s", "-c", "-o", "-N", "-R", "-W", "-l"], &[], o),
        "base64" => tools::read_files(args, &["-w", "--wrap", "-i", "-o"], &[], o),
        "shasum" => tools::read_files(args, &["-a", "--algorithm"], &[], o),
        "grep" | "egrep" | "fgrep" | "zgrep" | "ag" | "ack" => {
            tools::search(
                args,
                GREP_VALUE_OPTS,
                &["-e", "-f", "--regexp", "--file"],
                Recursion::ByFlag,
                o,
            );
        }
        "rg" => tools::search(
            args,
            RG_VALUE_OPTS,
            &["-e", "-f", "--regexp", "--file"],
            Recursion::Always,
            o,
        ),
        "yq" if args
            .first()
            .is_some_and(|w| matches!(w.text.as_str(), "e" | "eval" | "eval-all" | "ea")) =>
        {
            tools::search(
                &args[1..],
                &[
                    "-f",
                    "--from-file",
                    "-o",
                    "--output-format",
                    "-p",
                    "--input-format",
                    "-I",
                ],
                &["-f", "--from-file"],
                Recursion::Never,
                o,
            );
        }
        "jq" | "yq" | "gron" => tools::search(
            args,
            &[
                "-f",
                "--from-file",
                "--indent",
                "-e",
                "--expression",
                "-o",
                "--output-format",
                "-p",
                "--input-format",
                "-I",
            ],
            &["-f", "--from-file"],
            Recursion::Never,
            o,
        ),
        "awk" | "gawk" | "mawk" | "nawk" => tools::awk(args, o),
        "sed" | "gsed" => tools::sed(args, o),
        "perl" => tools::perl(args, o),
        "find" | "gfind" => tools::find(args, o),
        "fd" | "fdfind" => fd(args, o),
        "xargs" => tools::xargs(args, o),
        "locate" | "mlocate" | "plocate" => {
            o.class(CommandClass::Read);
        }

        // Removing, moving, copying.
        "rm" => tools::rm(args, o),
        "rmdir" | "unlink" | "shred" | "trash" | "trash-put" | "trash-cli" => {
            tools::delete_all(args, o)
        }
        "mv" => tools::mv(args, o),
        "cp" | "install" => tools::cp(args, o),
        "ln" => tools::ln(args, o),
        "touch" => tools::write_all(
            args,
            &["-r", "-t", "-d", "--reference", "--date", "--time"],
            o,
        ),
        "mkdir" => tools::write_all(args, &["-m", "--mode", "-Z", "--context"], o),
        "tee" => tools::write_all(args, &[], o),
        "truncate" => tools::write_all(args, &["-s", "--size", "-r", "--reference"], o),
        "mktemp" => tools::mktemp(args, o),
        "chmod" | "chown" | "chgrp" => tools::chmod(args, o),
        "dd" => tools::dd(args, o),
        "tar" | "gtar" | "bsdtar" => tools::tar(args, o),
        "unzip" => tools::unzip(args, o),
        "zip" => tools::zip(args, o),
        "gzip" | "gunzip" | "bzip2" | "bunzip2" | "xz" | "unxz" | "zstd" | "unzstd"
        | "compress" | "uncompress" => {
            tools::compress(args, o);
        }
        "7z" | "7za" | "7zr" => seven_zip(args, o),
        "split" | "csplit" | "patch" | "rename" | "mkfifo" | "mknod" => {
            o.class(CommandClass::Write).unknown_targets();
        }

        // Git.
        "git" => git::classify(args, o),

        // Network.
        "curl" => tools::curl(args, o),
        "wget" => tools::wget(args, o),
        "rsync" => tools::rsync(args, o),
        "scp" => tools::scp(args, o),
        "ssh" | "sftp" | "ftp" | "nc" | "ncat" | "netcat" | "telnet" | "ping" | "ping6"
        | "traceroute" | "dig" | "nslookup" | "host" | "whois" | "aws" | "gcloud" | "az"
        | "heroku" | "vercel" | "netlify" | "flyctl" | "fly" | "firebase" | "doctl" | "glab"
        | "wrangler" | "supabase" | "stripe" => {
            o.class(CommandClass::Network);
        }
        "gh" => run_sub(&GH, args, o),

        // Shells and interpreters.
        "sh" | "bash" | "zsh" | "dash" | "ksh" | "csh" | "tcsh" | "fish" | "ash" => {
            tools::shell(args, o)
        }
        "powershell" | "pwsh" => tools::powershell(args, o),
        "cmd" => tools::cmd_exe(args, o),
        "eval" => tools::eval(args, o),
        "alias" => tools::alias(args, o),
        "script" => tools::script(args, o),
        "su" | "runuser" => tools::su(args, o),
        "source" | "." => {
            o.and_unknown();
        }
        "python" | "py" | "node" | "nodejs" | "deno" | "ruby" | "php" | "lua" | "luajit"
        | "Rscript" | "julia" | "java" | "osascript" | "ts-node" | "tsx" => {
            tools::interpreter(args, o)
        }
        "cd" | "chdir" => tools::cd(args, o),
        "trap" => {
            // `trap 'script' SIGNAL`: the script runs later, in this directory.
            o.class(CommandClass::Read);
            let p = Parsed::new(args, &[]);
            if let Some(script) = p
                .pos
                .first()
                .filter(|w| w.is_exact() && !w.text.is_empty() && w.text != "-")
                && !p.has(&["-l", "-p"])
            {
                o.nested.push((crate::Dialect::Bash, script.text.clone()));
            }
        }
        "pushd" | "popd" => {
            o.class(CommandClass::Read);
            o.chdir = Some(crate::args::Chdir::To(None));
        }
        "npx" | "pnpx" | "bunx" | "uvx" => {
            o.class(CommandClass::Network).and_unknown();
        }

        // Package managers and build tools.
        "npm" => run_sub(&NPM, args, o),
        "yarn" => run_sub(&YARN, args, o),
        "pnpm" => run_sub(&PNPM, args, o),
        "bun" => run_sub(&BUN, args, o),
        "pip" | "pipx" => run_sub(&PIP, args, o),
        "uv" => run_sub(&UV, args, o),
        "poetry" => run_sub(&POETRY, args, o),
        "conda" | "mamba" | "micromamba" => run_sub(&CONDA, args, o),
        "gem" => run_sub(&GEM, args, o),
        "bundle" | "bundler" => run_sub(&BUNDLE, args, o),
        "composer" => run_sub(&COMPOSER, args, o),
        "brew" => run_sub(&BREW, args, o),
        "apt" | "apt-get" | "dnf" | "yum" | "zypper" | "apk" | "snap" | "port" | "choco"
        | "scoop" | "winget" => {
            run_sub(&SYSTEM_PACKAGES, args, o);
        }
        "apt-cache" | "dpkg-query" | "rpm-query" => {
            o.class(CommandClass::Read);
        }
        "dpkg" | "rpm" | "pacman" | "yay" => system_package_flags(args, o),
        "rustup" => run_sub(&RUSTUP, args, o),
        "docker" | "podman" | "nerdctl" | "docker-compose" | "podman-compose" => {
            run_sub(&DOCKER, args, o)
        }
        "kubectl" | "oc" | "k" => run_sub(&KUBECTL, args, o),
        "helm" => run_sub(&HELM, args, o),
        "terraform" | "tofu" => run_sub(&TERRAFORM, args, o),
        "dotnet" => run_sub(&DOTNET, args, o),
        "cargo" => cargo(args, o),
        "go" => go(args, o),
        "gcc" | "g++" | "cc" | "c++" | "clang" | "clang++" | "rustc" | "javac" | "tsc" => {
            tools::compiler(args, o);
        }

        // Linters and formatters.
        "mypy" | "flake8" | "pylint" | "pyright" | "shellcheck" | "hadolint" | "yamllint"
        | "markdownlint" | "golangci-lint" | "actionlint" | "cspell" | "tflint" | "xmllint"
        | "clippy-driver" | "bandit" | "semgrep" | "cloc" | "tokei" | "scc" => {
            o.class(CommandClass::Read);
        }
        "eslint" | "stylelint" => tools::format(args, &["--fix"], &[], o),
        "prettier" => tools::format(args, &["--write", "-w"], &[], o),
        "black" | "isort" | "rustfmt" | "stylua" | "swiftformat" | "dprint" => {
            tools::format(
                args,
                &[],
                &["--check", "--check-only", "-c", "--diff", "--dry-run"],
                o,
            );
        }
        "gofmt" | "shfmt" | "goimports" => tools::format(args, &["-w"], &[], o),
        "clang-format" | "autopep8" | "yapf" => tools::format(args, &["-i", "--in-place"], &[], o),
        "codespell" => tools::format(args, &["-w", "--write-changes"], &[], o),
        "biome" => tools::format(
            args.get(1..).unwrap_or_default(),
            &["--write", "--fix", "--apply"],
            &[],
            o,
        ),
        "ruff" => ruff(args, o),

        // Everything else: build tools, test runners, scripts, unknown programs.
        _ => {
            o.and_unknown();
            o.generic = true;
        }
    }
    out
}

fn fd(args: &[Word], out: &mut Outcome) {
    let exec = args
        .iter()
        .position(|a| matches!(a.text.as_str(), "-x" | "-X" | "--exec" | "--exec-batch"));
    match exec {
        Some(at) if at + 1 < args.len() => {
            // `{}`, `{/}`, `{.}`... stand for the matched path.
            let inner: Vec<Word> = args[at + 1..]
                .iter()
                .map(|w| {
                    if w.text.starts_with('{') && w.text.ends_with('}') && w.text.len() <= 4 {
                        Word::opaque(w.text.clone())
                    } else {
                        w.clone()
                    }
                })
                .collect();
            let (_, mut inner) = classify(&inner, false);
            inner.class(CommandClass::Read).and_unknown();
            *out = inner;
        }
        _ => {
            out.class(CommandClass::Read);
        }
    }
}

fn seven_zip(args: &[Word], out: &mut Outcome) {
    let p = Parsed::new(args, &["-p", "-o", "-t", "-x", "-i", "-m"]);
    match p.pos.first().map(|w| w.text.as_str()) {
        Some("x" | "e") => {
            out.class(CommandClass::Write);
            if let Some(archive) = p.pos.get(1) {
                out.target(archive, TargetOp::Read, false);
            }
            out.path(".", Kind::Exact, TargetOp::Write, true);
        }
        Some("a" | "u" | "d") => {
            out.class(CommandClass::Write);
            if let Some(archive) = p.pos.get(1) {
                out.target(archive, TargetOp::Write, false);
            }
            out.targets(p.pos.iter().copied().skip(2), TargetOp::Read, true);
        }
        Some("l" | "t" | "h" | "i") => {
            out.class(CommandClass::Read);
        }
        _ => {
            out.and_unknown();
        }
    }
}

fn ruff(args: &[Word], out: &mut Outcome) {
    match args.first().map(|w| w.text.as_str()) {
        Some("format") => tools::format(&args[1..], &[], &["--check", "--diff"], out),
        Some("check") => tools::format(&args[1..], &["--fix", "--unsafe-fixes"], &[], out),
        Some("version" | "--version" | "rule" | "config" | "linter") => {
            out.class(CommandClass::Read);
        }
        _ => {
            out.and_unknown();
        }
    }
}

fn system_package_flags(args: &[Word], out: &mut Outcome) {
    let query = args.first().is_some_and(|a| {
        a.text.starts_with("-Q")
            || matches!(
                a.text.as_str(),
                "-l" | "-L" | "-s" | "-S-info" | "-q" | "--list" | "--status" | "--listfiles"
            )
    });
    if query {
        out.class(CommandClass::Read);
    } else {
        out.class(CommandClass::Network).class(CommandClass::Write);
    }
}

// ---------------------------------------------------------------------------------------------
// Subcommand tables
// ---------------------------------------------------------------------------------------------

/// What a program without a subcommand does.
#[derive(Clone, Copy)]
enum Bare {
    Read,
    Install,
}

/// What an unlisted subcommand does.
#[derive(Clone, Copy)]
enum Other {
    Unknown,
    Network,
    Install,
}

/// How a program with subcommands is classified.
pub(crate) struct Sub {
    /// Options that take a value (so the subcommand is found correctly).
    value_opts: &'static [&'static str],
    /// Subcommands that only read.
    read: &'static [&'static str],
    /// Subcommands that talk to the network.
    network: &'static [&'static str],
    /// Subcommands that fetch and install: network plus a write of `writes`.
    install: &'static [&'static str],
    /// Subcommands that only write `writes`.
    modify: &'static [&'static str],
    /// The paths the install subcommands write; a trailing `/` marks a directory.
    writes: &'static [&'static str],
    /// The paths the modify subcommands write.
    modify_writes: &'static [&'static str],
    /// Flags that make an install affect the machine instead of the project (`-g`).
    global: &'static [&'static str],
    /// Subcommand groups (`docker image ...`): the next word decides, and unlisted ones are
    /// unknown.
    nested: &'static [&'static str],
    bare: Bare,
    other: Other,
}

const NO: &[&str] = &[];

const NPM: Sub = Sub {
    value_opts: &[
        "--prefix",
        "-C",
        "--workspace",
        "-w",
        "--registry",
        "--cache",
        "--userconfig",
        "--tag",
        "--otp",
        "--loglevel",
        "--scope",
        "--access",
    ],
    read: &[
        "ls",
        "list",
        "ll",
        "la",
        "outdated",
        "view",
        "info",
        "show",
        "v",
        "search",
        "s",
        "find",
        "whoami",
        "ping",
        "config",
        "c",
        "get",
        "help",
        "explain",
        "why",
        "doctor",
        "bin",
        "root",
        "prefix",
        "fund",
        "audit",
        "completion",
        "diff",
    ],
    network: &[
        "publish",
        "login",
        "logout",
        "adduser",
        "deprecate",
        "dist-tag",
        "owner",
        "access",
        "token",
        "unpublish",
        "team",
        "star",
        "unstar",
        "org",
        "hook",
        "profile",
    ],
    install: &[
        "install",
        "i",
        "add",
        "ci",
        "isntall",
        "in",
        "ins",
        "inst",
        "update",
        "up",
        "upgrade",
        "uninstall",
        "remove",
        "rm",
        "r",
        "un",
        "unlink",
        "link",
        "ln",
        "dedupe",
        "find-dupes",
        "prune",
        "rebuild",
        "rb",
        "install-test",
        "it",
        "install-ci-test",
        "cit",
    ],
    modify: &["init", "version", "pack", "set-script"],
    writes: &["node_modules/", "package.json", "package-lock.json"],
    modify_writes: &["package.json"],
    global: &["-g", "--global", "--location=global"],
    nested: NO,
    bare: Bare::Read,
    other: Other::Unknown,
};

const YARN: Sub = Sub {
    value_opts: &[
        "--cwd",
        "--registry",
        "--cache-folder",
        "--modules-folder",
        "--network-timeout",
    ],
    read: &[
        "list",
        "info",
        "why",
        "outdated",
        "audit",
        "config",
        "help",
        "bin",
        "check",
        "workspaces",
        "licenses",
        "versions",
        "cache",
    ],
    network: &[
        "publish", "login", "logout", "npm", "tag", "owner", "team", "global",
    ],
    install: &[
        "install", "add", "remove", "upgrade", "up", "import", "dedupe", "link", "unlink", "set",
    ],
    modify: &["init", "version", "pack"],
    writes: &["node_modules/", "package.json", "yarn.lock"],
    modify_writes: &["package.json"],
    global: NO,
    nested: NO,
    bare: Bare::Install,
    other: Other::Unknown,
};

const PNPM: Sub = Sub {
    value_opts: &["--dir", "-C", "--filter", "-F", "--registry", "--store-dir"],
    read: &[
        "list", "ls", "ll", "outdated", "why", "audit", "view", "info", "config", "get", "help",
        "root", "bin", "licenses",
    ],
    network: &["publish", "login", "logout"],
    install: &[
        "install",
        "i",
        "add",
        "remove",
        "rm",
        "uninstall",
        "un",
        "update",
        "up",
        "upgrade",
        "link",
        "ln",
        "unlink",
        "import",
        "prune",
        "rebuild",
        "rb",
        "dedupe",
        "fetch",
        "install-test",
        "it",
    ],
    modify: &["init", "pack"],
    writes: &["node_modules/", "package.json", "pnpm-lock.yaml"],
    modify_writes: &["package.json"],
    global: &["-g", "--global"],
    nested: NO,
    bare: Bare::Read,
    other: Other::Unknown,
};

const BUN: Sub = Sub {
    value_opts: &["--cwd", "--config", "-c"],
    read: &["pm", "help", "--version", "-v", "upgrade-check"],
    network: &["publish", "login"],
    install: &[
        "install", "i", "add", "a", "remove", "rm", "update", "link", "unlink",
    ],
    modify: &["init"],
    writes: &["node_modules/", "package.json", "bun.lockb"],
    modify_writes: &["package.json"],
    global: &["-g", "--global"],
    nested: NO,
    bare: Bare::Read,
    other: Other::Unknown,
};

pub(crate) const PIP: Sub = Sub {
    value_opts: &[
        "-r",
        "-c",
        "-e",
        "--requirement",
        "--constraint",
        "--editable",
        "-i",
        "--index-url",
        "--extra-index-url",
        "-f",
        "--find-links",
        "--target",
        "-t",
        "--prefix",
        "--root",
        "--src",
        "--proxy",
        "--cache-dir",
        "--progress-bar",
        "--log",
        "--python",
        "--platform",
        "--implementation",
        "--abi",
        "-d",
        "--dest",
        "--upgrade-strategy",
        "--timeout",
        "--retries",
        "--cert",
        "--client-cert",
        "--trusted-host",
        "--global-option",
        "--config-settings",
    ],
    read: &[
        "list",
        "freeze",
        "show",
        "check",
        "search",
        "config",
        "help",
        "inspect",
        "debug",
        "cache",
        "index",
        "hash",
        "completion",
        "version",
    ],
    network: NO,
    install: &[
        "install",
        "uninstall",
        "download",
        "wheel",
        "inject",
        "upgrade",
        "reinstall",
    ],
    modify: NO,
    writes: NO,
    modify_writes: NO,
    global: NO,
    nested: NO,
    bare: Bare::Read,
    other: Other::Unknown,
};

const UV: Sub = Sub {
    value_opts: &[
        "--directory",
        "--project",
        "--python",
        "-p",
        "--index-url",
        "--extra-index-url",
        "--cache-dir",
        "--config-file",
    ],
    read: &["tree", "version", "help", "cache", "self"],
    network: NO,
    install: &[
        "sync", "add", "remove", "lock", "init", "venv", "pip", "tool", "python", "build",
        "publish",
    ],
    modify: NO,
    writes: &[".venv/"],
    modify_writes: NO,
    global: NO,
    nested: NO,
    bare: Bare::Read,
    other: Other::Unknown,
};

const POETRY: Sub = Sub {
    value_opts: &["--directory", "-C", "--project", "-P"],
    read: &[
        "show", "check", "version", "about", "config", "list", "search", "help", "env",
    ],
    network: &["publish"],
    install: &["install", "add", "remove", "update", "lock", "sync"],
    modify: &["init", "new"],
    writes: &["pyproject.toml", "poetry.lock"],
    modify_writes: &["pyproject.toml"],
    global: NO,
    nested: NO,
    bare: Bare::Read,
    other: Other::Unknown,
};

const CONDA: Sub = Sub {
    value_opts: &[
        "-n",
        "--name",
        "-p",
        "--prefix",
        "-c",
        "--channel",
        "-f",
        "--file",
    ],
    read: &[
        "list",
        "info",
        "search",
        "config",
        "help",
        "env",
        "--version",
        "-V",
        "doctor",
        "compare",
    ],
    network: NO,
    install: &[
        "install",
        "create",
        "remove",
        "uninstall",
        "update",
        "upgrade",
        "clean",
    ],
    modify: NO,
    writes: NO,
    modify_writes: NO,
    global: NO,
    nested: NO,
    bare: Bare::Read,
    other: Other::Unknown,
};

const GEM: Sub = Sub {
    value_opts: &[
        "-s",
        "--source",
        "-v",
        "--version",
        "-i",
        "--install-dir",
        "-n",
        "--bindir",
        "--platform",
    ],
    read: &[
        "list",
        "query",
        "search",
        "info",
        "help",
        "env",
        "contents",
        "specification",
        "which",
        "environment",
        "outdated",
    ],
    network: &["push", "yank", "owner", "signin", "signout"],
    install: &[
        "install",
        "uninstall",
        "update",
        "cleanup",
        "fetch",
        "pristine",
    ],
    modify: NO,
    writes: NO,
    modify_writes: NO,
    global: NO,
    nested: NO,
    bare: Bare::Read,
    other: Other::Unknown,
};

const BUNDLE: Sub = Sub {
    value_opts: &["--gemfile", "--path", "--with", "--without", "--jobs", "-j"],
    read: &[
        "list", "show", "info", "check", "outdated", "config", "help", "version", "platform", "env",
    ],
    network: NO,
    install: &[
        "install", "update", "add", "remove", "lock", "cache", "package", "pristine",
    ],
    modify: NO,
    writes: &["Gemfile.lock"],
    modify_writes: NO,
    global: NO,
    nested: NO,
    bare: Bare::Install,
    other: Other::Unknown,
};

const COMPOSER: Sub = Sub {
    value_opts: &["-d", "--working-dir"],
    read: &[
        "show",
        "info",
        "outdated",
        "validate",
        "help",
        "list",
        "about",
        "licenses",
        "status",
        "diagnose",
        "check-platform-reqs",
        "why",
        "why-not",
        "depends",
        "prohibits",
        "browse",
    ],
    network: &["archive"],
    install: &[
        "install",
        "update",
        "require",
        "remove",
        "upgrade",
        "dump-autoload",
        "dumpautoload",
        "reinstall",
    ],
    modify: &["init"],
    writes: &["vendor/", "composer.json", "composer.lock"],
    modify_writes: &["composer.json"],
    global: &["global"],
    nested: NO,
    bare: Bare::Read,
    other: Other::Unknown,
};

const BREW: Sub = Sub {
    value_opts: NO,
    read: &[
        "list",
        "ls",
        "info",
        "search",
        "outdated",
        "deps",
        "leaves",
        "config",
        "doctor",
        "--version",
        "--prefix",
        "--cellar",
        "--cache",
        "--repository",
        "home",
        "desc",
        "uses",
        "missing",
        "commands",
        "help",
    ],
    network: &["fetch"],
    install: &[
        "install",
        "uninstall",
        "remove",
        "rm",
        "reinstall",
        "upgrade",
        "update",
        "cleanup",
        "tap",
        "untap",
        "link",
        "unlink",
        "pin",
        "unpin",
        "autoremove",
        "services",
        "bundle",
    ],
    modify: NO,
    writes: NO,
    modify_writes: NO,
    global: NO,
    nested: NO,
    bare: Bare::Read,
    other: Other::Unknown,
};

const SYSTEM_PACKAGES: Sub = Sub {
    value_opts: &["-o", "-c", "-t", "-y-none"],
    read: &[
        "list",
        "search",
        "show",
        "info",
        "policy",
        "depends",
        "rdepends",
        "madison",
        "provides",
        "repolist",
        "--version",
        "-v",
        "help",
        "changelog",
        "download-only",
    ],
    network: NO,
    install: &[
        "install",
        "remove",
        "purge",
        "autoremove",
        "upgrade",
        "update",
        "dist-upgrade",
        "full-upgrade",
        "add",
        "del",
        "reinstall",
        "erase",
        "distro-sync",
        "refresh",
        "clean",
        "autoclean",
        "refresh",
        "uninstall",
        "build-dep",
        "source",
    ],
    modify: NO,
    writes: NO,
    modify_writes: NO,
    global: NO,
    nested: NO,
    bare: Bare::Read,
    other: Other::Unknown,
};

const RUSTUP: Sub = Sub {
    value_opts: &["--toolchain"],
    read: &["show", "which", "doc", "help", "--version", "-V", "check"],
    network: NO,
    install: &[
        "install",
        "update",
        "default",
        "toolchain",
        "target",
        "component",
        "self",
        "override",
        "set",
        "uninstall",
    ],
    modify: NO,
    writes: NO,
    modify_writes: NO,
    global: NO,
    nested: NO,
    bare: Bare::Read,
    other: Other::Install,
};

const DOCKER: Sub = Sub {
    value_opts: &[
        "-H",
        "--host",
        "-c",
        "--context",
        "--config",
        "-l",
        "--log-level",
        "-f",
        "--file",
        "-p",
        "--project-name",
        "--env-file",
        "--profile",
    ],
    read: &[
        "ps", "images", "logs", "inspect", "version", "info", "stats", "top", "port", "diff",
        "history", "search", "events", "help", "ls", "list", "df", "config",
    ],
    network: &["pull", "push", "login", "logout"],
    install: NO,
    modify: NO,
    writes: NO,
    modify_writes: NO,
    global: NO,
    nested: &[
        "image",
        "container",
        "network",
        "volume",
        "system",
        "context",
        "builder",
        "buildx",
        "manifest",
        "compose",
        "stack",
        "service",
        "node",
        "secret",
        "plugin",
        "trust",
    ],
    bare: Bare::Read,
    other: Other::Unknown,
};

const KUBECTL: Sub = Sub {
    value_opts: &[
        "-n",
        "--namespace",
        "--context",
        "--kubeconfig",
        "-l",
        "--selector",
        "-o",
        "--output",
        "-f",
        "--filename",
        "-c",
        "--container",
        "--cluster",
        "--user",
        "-s",
        "--server",
    ],
    read: &[
        "get",
        "describe",
        "logs",
        "top",
        "explain",
        "version",
        "config",
        "cluster-info",
        "api-resources",
        "api-versions",
        "diff",
        "events",
        "auth",
        "wait",
        "help",
    ],
    network: NO,
    install: NO,
    modify: NO,
    writes: NO,
    modify_writes: NO,
    global: NO,
    nested: NO,
    bare: Bare::Read,
    other: Other::Unknown,
};

const HELM: Sub = Sub {
    value_opts: &[
        "-n",
        "--namespace",
        "--kube-context",
        "--kubeconfig",
        "-f",
        "--values",
        "--set",
        "--version",
    ],
    read: &[
        "list",
        "ls",
        "status",
        "get",
        "show",
        "search",
        "template",
        "lint",
        "version",
        "env",
        "history",
        "help",
        "verify",
        "completion",
    ],
    network: &["pull", "repo", "registry"],
    install: NO,
    modify: NO,
    writes: NO,
    modify_writes: NO,
    global: NO,
    nested: NO,
    bare: Bare::Read,
    other: Other::Unknown,
};

const TERRAFORM: Sub = Sub {
    value_opts: &["-chdir"],
    read: &[
        "plan",
        "show",
        "validate",
        "version",
        "output",
        "providers",
        "graph",
        "workspace",
        "state",
        "help",
        "-version",
        "-help",
    ],
    network: NO,
    install: &["init", "get"],
    modify: NO,
    writes: &[".terraform/", ".terraform.lock.hcl"],
    modify_writes: NO,
    global: NO,
    nested: NO,
    bare: Bare::Read,
    other: Other::Unknown,
};

const DOTNET: Sub = Sub {
    value_opts: NO,
    read: &[
        "--version",
        "--info",
        "--list-sdks",
        "--list-runtimes",
        "help",
        "--help",
        "list",
        "nuget",
    ],
    network: NO,
    install: NO,
    modify: NO,
    writes: NO,
    modify_writes: NO,
    global: NO,
    nested: NO,
    bare: Bare::Read,
    other: Other::Unknown,
};

const GH: Sub = Sub {
    value_opts: &["-R", "--repo", "-H", "--hostname"],
    read: &["help", "--version", "version", "completion", "alias"],
    network: NO,
    install: NO,
    modify: NO,
    writes: NO,
    modify_writes: NO,
    global: NO,
    nested: NO,
    bare: Bare::Read,
    other: Other::Network,
};

/// Adds the paths a table says its install and modify subcommands write; a trailing `/` marks a
/// directory.
fn write_paths(paths: &[&str], out: &mut Outcome) {
    for path in paths {
        let dir = path.ends_with('/');
        out.path(
            path.trim_end_matches('/'),
            Kind::Exact,
            TargetOp::Write,
            dir,
        );
    }
}

/// Classifies `<program> <subcommand> ...` with a [`Sub`] table.
pub(crate) fn run_sub(spec: &Sub, args: &[Word], out: &mut Outcome) {
    let p = Parsed::new(args, spec.value_opts);
    let positionals = p.positionals();
    let Some(first) = positionals.first() else {
        if p.has(&["--version", "-v", "-V", "--help", "-h"]) {
            out.class(CommandClass::Read);
            return;
        }
        match spec.bare {
            Bare::Read => out.class(CommandClass::Read),
            Bare::Install => out.class(CommandClass::Network).class(CommandClass::Write),
        };
        if matches!(spec.bare, Bare::Install) {
            write_paths(spec.writes, out);
        }
        return;
    };
    if !first.is_exact() {
        out.and_unknown();
        return;
    }
    let sub = first.text.as_str();
    if spec.nested.contains(&sub) {
        // `docker image ls`, `docker compose ps`: the next word decides.
        let next = positionals
            .get(1)
            .filter(|w| w.is_exact())
            .map(|w| w.text.as_str());
        if next.is_some_and(|n| {
            matches!(
                n,
                "ls" | "list"
                    | "ps"
                    | "inspect"
                    | "logs"
                    | "top"
                    | "history"
                    | "df"
                    | "info"
                    | "version"
                    | "config"
                    | "images"
                    | "events"
                    | "port"
            )
        }) {
            out.class(CommandClass::Read);
        } else {
            out.and_unknown();
        }
    } else if spec.read.contains(&sub) {
        out.class(CommandClass::Read);
    } else if spec.network.contains(&sub) {
        out.class(CommandClass::Network);
    } else if spec.install.contains(&sub) {
        out.class(CommandClass::Network).class(CommandClass::Write);
        if !p.has(spec.global)
            && !positionals
                .iter()
                .any(|w| spec.global.contains(&w.text.as_str()))
        {
            write_paths(spec.writes, out);
        }
    } else if spec.modify.contains(&sub) {
        out.class(CommandClass::Write);
        write_paths(spec.modify_writes, out);
    } else {
        match spec.other {
            Other::Unknown => out.and_unknown(),
            Other::Network => out.class(CommandClass::Network),
            Other::Install => out.class(CommandClass::Network).class(CommandClass::Write),
        };
    }
}

// ---------------------------------------------------------------------------------------------
// cargo and go
// ---------------------------------------------------------------------------------------------

fn cargo(args: &[Word], out: &mut Outcome) {
    let p = Parsed::new(
        args,
        &[
            "--manifest-path",
            "-p",
            "--package",
            "--target",
            "--target-dir",
            "--features",
            "-F",
            "--profile",
            "-j",
            "--jobs",
            "--color",
            "--config",
            "-Z",
            "--bin",
            "--example",
            "--test",
            "--bench",
            "--message-format",
            "--registry",
            "--index",
            "--root",
            "--git",
            "--branch",
            "--tag",
            "--rev",
            "--path",
            "-C",
            "--exclude",
            "--lockfile-path",
        ],
    );
    // `cargo +nightly build`.
    let mut positional = p.positionals().iter().filter(|w| !w.text.starts_with('+'));
    let Some(sub) = positional.next() else {
        out.class(CommandClass::Read);
        return;
    };
    if !sub.is_exact() {
        out.and_unknown();
        return;
    }
    let target_dir = p.value(&["--target-dir"]);
    let build_dir = |out: &mut Outcome| {
        match &target_dir {
            Some(dir) => out.target(dir, TargetOp::Write, true),
            None => out.path("target", Kind::Exact, TargetOp::Write, true),
        };
    };
    match sub.text.as_str() {
        "build" | "b" | "check" | "c" | "doc" | "d" | "rustc" | "rustdoc" | "clippy" | "expand" => {
            out.class(CommandClass::Write);
            build_dir(out);
            if p.has(&["--fix"]) {
                out.path(".", Kind::Exact, TargetOp::Write, true);
            }
        }
        "test" | "t" | "run" | "r" | "bench" | "nextest" | "tarpaulin" | "miri" | "llvm-cov"
        | "fuzz" | "watch" => {
            out.class(CommandClass::Write);
            build_dir(out);
            out.and_unknown();
        }
        "fix" => {
            out.class(CommandClass::Write);
            build_dir(out);
            out.path(".", Kind::Exact, TargetOp::Write, true);
        }
        "fmt" => {
            if p.has(&["--check"]) || p.pos.iter().any(|w| w.text == "--check") {
                out.class(CommandClass::Read);
            } else {
                out.class(CommandClass::Write);
                out.path(".", Kind::Exact, TargetOp::Write, true);
            }
        }
        "clean" => {
            out.class(CommandClass::Delete);
            match &target_dir {
                Some(dir) => out.target(dir, TargetOp::Delete, true),
                None => out.path("target", Kind::Exact, TargetOp::Delete, true),
            };
        }
        "add" | "remove" | "rm" => {
            out.class(CommandClass::Write).class(CommandClass::Network);
            out.path("Cargo.toml", Kind::Exact, TargetOp::Write, false);
            out.path("Cargo.lock", Kind::Exact, TargetOp::Write, false);
        }
        "update" | "generate-lockfile" | "fetch" => {
            out.class(CommandClass::Write).class(CommandClass::Network);
            out.path("Cargo.lock", Kind::Exact, TargetOp::Write, false);
        }
        "new" | "init" => {
            out.class(CommandClass::Write);
            match positional.next() {
                Some(dir) => {
                    out.target(dir, TargetOp::Write, true);
                }
                None if sub.text == "init" => {
                    out.path("Cargo.toml", Kind::Exact, TargetOp::Write, false);
                    out.path("src", Kind::Exact, TargetOp::Write, true);
                }
                None => {
                    out.unknown_targets();
                }
            }
        }
        "vendor" => {
            out.class(CommandClass::Write).class(CommandClass::Network);
            out.path("vendor", Kind::Exact, TargetOp::Write, true);
        }
        "install" | "uninstall" => {
            out.class(CommandClass::Network).class(CommandClass::Write);
        }
        "publish" | "login" | "logout" | "search" | "owner" | "yank" => {
            out.class(CommandClass::Network);
        }
        "metadata" | "tree" | "locate-project" | "pkgid" | "verify-project" | "version"
        | "help" | "audit" | "deny" | "outdated" | "machete" | "bloat" | "udeps" | "msrv"
        | "semver-checks" | "read-manifest" => {
            out.class(CommandClass::Read);
        }
        _ => {
            out.and_unknown();
        }
    }
}

fn go(args: &[Word], out: &mut Outcome) {
    let p = Parsed::new(
        args,
        &[
            "-o",
            "-C",
            "-p",
            "-tags",
            "-ldflags",
            "-gcflags",
            "-run",
            "-bench",
            "-count",
            "-timeout",
            "-mod",
            "-modfile",
            "-cover",
            "-coverprofile",
            "-race",
        ],
    );
    let Some(sub) = p.positionals().first() else {
        out.class(CommandClass::Read);
        return;
    };
    match sub.text.as_str() {
        "build" => {
            out.class(CommandClass::Write);
            match p.value(&["-o"]) {
                Some(file) => out.target(&file, TargetOp::Write, false),
                None => out.unknown_targets(),
            };
        }
        "test" | "run" | "generate" | "tool" | "work" => {
            out.and_unknown();
        }
        "vet" | "list" | "doc" | "env" | "version" | "help" => {
            out.class(CommandClass::Read);
        }
        "fmt" => {
            out.class(CommandClass::Write);
            out.path(".", Kind::Exact, TargetOp::Write, true);
        }
        "get" => {
            out.class(CommandClass::Network).class(CommandClass::Write);
            out.path("go.mod", Kind::Exact, TargetOp::Write, false);
            out.path("go.sum", Kind::Exact, TargetOp::Write, false);
        }
        "install" => {
            out.class(CommandClass::Network).class(CommandClass::Write);
        }
        "clean" => {
            out.class(CommandClass::Delete).unknown_targets();
        }
        "mod" => match p.positionals().get(1).map(|w| w.text.as_str()) {
            Some("tidy") => {
                out.class(CommandClass::Network).class(CommandClass::Write);
                out.path("go.mod", Kind::Exact, TargetOp::Write, false);
                out.path("go.sum", Kind::Exact, TargetOp::Write, false);
            }
            Some("download") => {
                out.class(CommandClass::Network);
            }
            Some("vendor") => {
                out.class(CommandClass::Network).class(CommandClass::Write);
                out.path("vendor", Kind::Exact, TargetOp::Write, true);
            }
            Some("init" | "edit") => {
                out.class(CommandClass::Write);
                out.path("go.mod", Kind::Exact, TargetOp::Write, false);
            }
            Some("graph" | "why" | "verify") => {
                out.class(CommandClass::Read);
            }
            _ => {
                out.and_unknown();
            }
        },
        _ => {
            out.and_unknown();
        }
    }
}
