//! ACS's self-protection rules: shell commands an agent may never run because they would switch
//! ACS off, act as the human, or skip its git hooks, whatever the policy says and whatever the
//! gate state (decision 0026).
//!
//! The rules read the effective words of each [`SimpleCommand`] (wrappers such as `sudo` and
//! `env` are already removed by the shell analysis, and the scripts of `sh -c`, `eval`, aliases
//! and shells fed on stdin are analyzed as commands of their own), so they see through those.
//! What they cannot see (a script file, an interpreter, a variable naming the program) is
//! documented in `docs/security/overrides.md`.

use crate::gate::{CommandClass, OutsideSeal, SimpleCommand};

/// `acs` subcommands an agent may run from its shell: they only read, or only share claims with
/// the remote. `doctor` is listed, but `doctor --fix` is not allowed. `lore` is decided per
/// action in [`denied_sub`]: `list`, `show` and `stale` are allowed, `review` is not.
const READ_ONLY: &[&str] = &["status", "sync", "doctor", "help"];

/// The `acs` subcommands that change state. When `acs` appears as an argument of another
/// program (`xargs acs off`, `cargo run --bin acs -- off`), only these count, so that
/// `grep -r acs src` is not mistaken for running ACS.
const STATEFUL: &[&str] = &[
    "hook", "init", "claim", "release", "off", "on", "plugin", "mcp", "git-hook", "override",
    "watch",
];

/// Paths inside the repository that a command with unknown effects must not mention: ACS's
/// policy, git hooks and config, and harness settings. Sealed globs protect them from commands
/// whose targets are known.
const IN_REPO_MENTIONS: &[&str] = &[
    ".acs/",
    ".git/hooks",
    ".git/config",
    ".claude/settings",
    ".husky/",
];

/// Why a command breaks a self-protection rule.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Breach {
    /// Runs an `acs` subcommand that changes state, shown as `acs <subcommand>`.
    AcsCli(String),
    /// Skips or redirects git hooks (`--no-verify`, `core.hooksPath`).
    GitHooks(String),
    /// Starts an agent harness with its hooks or the ACS plugin turned off.
    Harness(String),
}

/// The first rule `command` breaks, if any.
pub(super) fn check(command: &SimpleCommand) -> Option<Breach> {
    let words: Vec<&str> = command.words.iter().map(String::as_str).collect();
    acs_cli(&words)
        .map(Breach::AcsCli)
        .or_else(|| git_hooks(command, &words).map(Breach::GitHooks))
        .or_else(|| harness(&words).map(Breach::Harness))
}

/// The protected location a command with unknown effects mentions, if any: its label for an
/// outside seal, or the in-repository path.
pub(super) fn mentioned(command: &SimpleCommand, outside: &[OutsideSeal]) -> Option<String> {
    if !command.classes.contains(&CommandClass::Unknown) {
        return None;
    }
    for word in &command.words {
        if let Some(seal) = outside.iter().find(|seal| seal.mentioned_in(word)) {
            return Some(seal.label.clone());
        }
        let folded = word.replace('\\', "/").to_ascii_lowercase();
        if let Some(path) = IN_REPO_MENTIONS.iter().find(|p| folded.contains(*p)) {
            return Some(format!("`{path}`"));
        }
    }
    None
}

/// The program name of a word: its last path component, lowercased, without a Windows
/// executable extension.
fn program(word: &str) -> String {
    let base = word.rsplit(['/', '\\']).next().unwrap_or(word);
    let lower = base.to_ascii_lowercase();
    for ext in [".exe", ".cmd", ".bat", ".ps1"] {
        if let Some(stem) = lower.strip_suffix(ext) {
            return stem.to_owned();
        }
    }
    lower
}

/// The first word after `from` that is not an option.
fn next_operand<'a>(words: &[&'a str], from: usize) -> Option<&'a str> {
    words
        .get(from..)?
        .iter()
        .copied()
        .find(|w| !w.starts_with('-'))
}

/// A state-changing `acs` invocation: returns the command as shown to the agent.
fn acs_cli(words: &[&str]) -> Option<String> {
    let first = words.first()?;
    if program(first) == "acs" {
        let sub = next_operand(words, 1)?;
        return denied_sub(sub, &words[1..], true).then(|| shown(sub, &words[1..]));
    }
    if program(first) == "cargo" && words.contains(&"run") && runs_acs_package(words) {
        let at = words.iter().position(|w| *w == "--")?;
        let sub = next_operand(words, at + 1)?;
        return denied_sub(sub, &words[at + 1..], true).then(|| shown(sub, &words[at + 1..]));
    }
    for (index, word) in words.iter().enumerate().skip(1) {
        if program(word) == "acs"
            && let Some(sub) = next_operand(words, index + 1)
            && denied_sub(sub, &words[index + 1..], false)
        {
            return Some(shown(sub, &words[index + 1..]));
        }
    }
    None
}

/// The denied invocation as shown to the agent: `acs <sub>`, plus the action for `acs lore`.
fn shown(sub: &str, rest: &[&str]) -> String {
    if sub.eq_ignore_ascii_case("lore")
        && let Some(action) = rest
            .iter()
            .position(|w| w.eq_ignore_ascii_case("lore"))
            .and_then(|at| next_operand(rest, at + 1))
    {
        return format!("acs lore {action}");
    }
    format!("acs {sub}")
}

/// Whether running `acs <sub> <rest>` is denied. As the program itself (`direct`), anything but a
/// read-only subcommand is; as an argument of another program, only a known stateful one.
fn denied_sub(sub: &str, rest: &[&str], direct: bool) -> bool {
    let sub = sub.to_ascii_lowercase();
    if sub == "doctor" {
        return rest.contains(&"--fix");
    }
    if sub == "lore" {
        // `acs lore list|show|stale` only read; `acs lore review` turns proposals into commits,
        // which is the owner's call (decision 0029). A bare `acs lore` prints help.
        let action = rest
            .iter()
            .position(|w| w.eq_ignore_ascii_case("lore"))
            .and_then(|at| next_operand(rest, at + 1))
            .map(str::to_ascii_lowercase);
        return !matches!(
            action.as_deref(),
            None | Some("list" | "show" | "stale" | "help")
        );
    }
    if direct {
        !READ_ONLY.contains(&sub.as_str())
    } else {
        STATEFUL.contains(&sub.as_str())
    }
}

/// `cargo run -p acs-cli`, `cargo run --bin acs` and their `=` spellings.
fn runs_acs_package(words: &[&str]) -> bool {
    words.windows(2).any(|pair| {
        matches!(pair, [flag, value] if matches!(*flag, "-p" | "--package" | "--bin")
            && matches!(*value, "acs-cli" | "acs"))
    }) || words.iter().any(|w| {
        matches!(
            *w,
            "--package=acs-cli" | "--bin=acs" | "-pacs-cli" | "--package=acs"
        )
    })
}

/// Skipping or redirecting git hooks: `--no-verify` (and `git commit -n`), or setting
/// `core.hooksPath` for one command or in the config.
fn git_hooks(command: &SimpleCommand, words: &[&str]) -> Option<String> {
    let git = command.git.as_ref()?;
    let shown = |what: &str| format!("git {} {what}", git.subcommand);
    if words.contains(&"--no-verify") {
        return Some(shown("--no-verify"));
    }
    if words
        .iter()
        .any(|w| w.to_ascii_lowercase().contains("core.hookspath="))
    {
        return Some("git -c core.hooksPath=...".to_owned());
    }
    if git.subcommand == "commit" && commit_skips_hooks(words) {
        return Some(shown("-n"));
    }
    if git.subcommand == "config" && config_sets_hooks_path(words) {
        return Some(shown("core.hooksPath"));
    }
    None
}

/// `git commit -n`, alone or in a cluster of short flags (`-an`), before any `--`.
fn commit_skips_hooks(words: &[&str]) -> bool {
    /// Short options of `git commit` that take a value (attached or as the next word).
    const VALUED: &[char] = &['m', 'F', 'c', 'C', 't', 'S', 'u'];
    let start = words
        .iter()
        .position(|w| *w == "commit")
        .map_or(0, |i| i + 1);
    let mut skip_next = false;
    for word in &words[start..] {
        if std::mem::take(&mut skip_next) {
            continue;
        }
        if *word == "--" {
            break;
        }
        let Some(flags) = word.strip_prefix('-').filter(|f| !f.starts_with('-')) else {
            continue;
        };
        for (at, flag) in flags.char_indices() {
            if flag == 'n' {
                return true;
            }
            if VALUED.contains(&flag) {
                skip_next = at + flag.len_utf8() == flags.len() && !matches!(flag, 'S' | 'u');
                break;
            }
        }
    }
    false
}

/// `git config` that writes or removes `core.hooksPath` (reading it is fine).
fn config_sets_hooks_path(words: &[&str]) -> bool {
    let start = words
        .iter()
        .position(|w| *w == "config")
        .map_or(0, |i| i + 1);
    let rest = &words[start..];
    let Some(key_at) = rest
        .iter()
        .position(|w| w.eq_ignore_ascii_case("core.hooksPath"))
    else {
        return false;
    };
    let writes_flag = rest.iter().any(|w| {
        matches!(
            *w,
            "--unset" | "--unset-all" | "--replace-all" | "--add" | "--edit" | "-e"
        )
    });
    let verb = rest
        .iter()
        .take(key_at)
        .find(|w| !w.starts_with('-'))
        .is_some_and(|w| matches!(*w, "set" | "unset"));
    let has_value = rest[key_at + 1..].iter().any(|w| !w.starts_with('-'));
    writes_flag || verb || has_value
}

/// An agent harness started with its hooks or the ACS plugin turned off (issue 13).
fn harness(words: &[&str]) -> Option<String> {
    let name = program(words.first()?);
    let flags_off: &[&str] = match name.as_str() {
        "claude" => &["--bare", "--settings", "--setting-sources"],
        "agy" | "antigravity" => &["--settings"],
        _ => return None,
    };
    for word in &words[1..] {
        let flag = word.split_once('=').map_or(*word, |(flag, _)| flag);
        if flags_off.contains(&flag) {
            return Some(format!("{name} {flag}"));
        }
    }
    let sub = next_operand(words, 1);
    let action = sub.and_then(|s| {
        let at = words.iter().position(|w| *w == s)?;
        next_operand(words, at + 1)
    });
    match (sub, action) {
        (
            Some("plugin" | "plugins"),
            Some(action @ ("disable" | "uninstall" | "remove" | "rm")),
        ) => Some(format!("{name} plugin {action}")),
        (Some("config"), Some(action @ ("set" | "add" | "remove"))) if name == "claude" => {
            Some(format!("{name} config {action}"))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gate::GitInvocation;

    fn cmd(line: &str) -> SimpleCommand {
        let words: Vec<String> = line.split_whitespace().map(str::to_owned).collect();
        let git = (words.first().map(String::as_str) == Some("git")).then(|| GitInvocation {
            subcommand: words
                .iter()
                .skip(1)
                .find(|w| !w.starts_with('-') && !w.contains('='))
                .cloned()
                .unwrap_or_default(),
            ..GitInvocation::default()
        });
        SimpleCommand {
            words,
            classes: vec![CommandClass::Unknown],
            targets: Vec::new(),
            targets_known: false,
            git,
        }
    }

    fn breach(line: &str) -> Option<Breach> {
        check(&cmd(line))
    }

    #[test]
    fn agents_may_only_run_read_only_acs_subcommands() {
        for line in [
            "acs",
            "acs --version",
            "acs status --json",
            "acs sync",
            "acs doctor",
            "acs help override",
            "acs lore",
            "acs lore list --json",
            "acs lore show src/a.rs",
            "acs lore stale --all",
            "acs lore help review",
            "grep -rn acs src",
            "rg acs off.rs",
            "cargo run -p acs-cli -- status",
            "cargo build -p acs-cli",
        ] {
            assert_eq!(breach(line), None, "{line}");
        }
        for (line, shown) in [
            ("acs off", "acs off"),
            ("acs on", "acs on"),
            ("acs override --path x --once", "acs override"),
            ("acs claim a.rs", "acs claim"),
            ("acs release --all", "acs release"),
            ("acs init --yes", "acs init"),
            ("acs plugin install claude-code", "acs plugin"),
            ("acs hook --canonical pre_tool", "acs hook"),
            ("acs mcp", "acs mcp"),
            ("acs doctor --fix", "acs doctor"),
            ("acs lore review", "acs lore review"),
            ("acs lore review --accept prop_1", "acs lore review"),
            ("acs lore --json review --list", "acs lore review"),
            ("acs lore future", "acs lore future"),
            ("xargs acs lore review --reject", "acs lore review"),
            ("cargo run -p acs-cli -- lore review", "acs lore review"),
            ("acs future-command", "acs future-command"),
            ("/usr/local/bin/acs off", "acs off"),
            ("C:\\tools\\ACS.EXE off", "acs off"),
            ("./target/debug/acs.exe override", "acs override"),
            ("xargs acs release", "acs release"),
            ("cargo run -p acs-cli -- off", "acs off"),
            ("cargo run --bin=acs -- override --path x", "acs override"),
        ] {
            assert_eq!(
                breach(line),
                Some(Breach::AcsCli(shown.to_owned())),
                "{line}"
            );
        }
    }

    #[test]
    fn skipping_or_redirecting_git_hooks_is_caught() {
        for line in [
            "git commit --no-verify -m x",
            "git push --no-verify",
            "git commit -n -m x",
            "git commit -anm x",
            "git -c core.hooksPath=/dev/null commit -m x",
            "git config core.hooksPath /tmp/none",
            "git config --global core.hooksPath x",
            "git config --unset core.hooksPath",
            "git config set core.hooksPath x",
        ] {
            assert!(matches!(breach(line), Some(Breach::GitHooks(_))), "{line}");
        }
        for line in [
            "git commit -m -n",
            "git commit -m x -- -n",
            "git config core.hooksPath",
            "git config --get core.hooksPath",
            "git log -n 3",
            "git commit -am x",
        ] {
            assert_eq!(breach(line), None, "{line}");
        }
    }

    #[test]
    fn harnesses_with_hooks_off_are_caught() {
        for line in [
            "claude --bare -p hi",
            "claude -p hi --settings {\"disableAllHooks\":true}",
            "claude --settings=x.json -p hi",
            "claude --setting-sources user -p hi",
            "claude plugin disable acs@acs-plugins",
            "claude plugin uninstall acs",
            "claude config set -g disableAllHooks true",
            "agy --settings s.json -p hi",
            "agy plugin disable acs",
        ] {
            assert!(matches!(breach(line), Some(Breach::Harness(_))), "{line}");
        }
        for line in [
            "claude -p hi",
            "claude --plugin-dir x -p hi",
            "claude plugin list",
            "agy -p hi",
        ] {
            assert_eq!(breach(line), None, "{line}");
        }
    }

    #[test]
    fn unknown_commands_must_not_mention_protected_locations() {
        let seals = [OutsideSeal {
            path: "C:\\Users\\me\\.claude\\settings.json".to_owned(),
            label: "Claude Code user settings".to_owned(),
            home_relative: Some(".claude/settings.json".to_owned()),
        }];
        let python = |code: &str| {
            let mut c = cmd("python -c");
            c.words.push(code.to_owned());
            c
        };
        assert_eq!(
            mentioned(
                &python("open(os.path.expanduser('~/.claude/settings.json'),'w')"),
                &seals
            ),
            Some("Claude Code user settings".to_owned())
        );
        assert_eq!(
            mentioned(&python("open('c:/users/ME/.claude/settings.json')"), &seals),
            Some("Claude Code user settings".to_owned())
        );
        assert_eq!(
            mentioned(&python("open('.acs/policy/guards.toml','w')"), &seals),
            Some("`.acs/`".to_owned())
        );
        assert_eq!(mentioned(&python("print(1)"), &seals), None);
        let mut known = cmd("cat .acs/policy/guards.toml");
        known.classes = vec![CommandClass::Read];
        assert_eq!(mentioned(&known, &seals), None, "known effects are fine");
    }
}
