//! Properties that must hold for any input: no panics, and the shape guarantees the contract in
//! `acs_core::gate` makes. The corpus and its prefixes serve as inputs, plus hostile ones.

mod common;

use acs_core::gate::{CommandClass, ShellAnalysis};
use acs_shell::{Dialect, analyze};
use common::{CORPUS, parse_corpus};

fn check(command: &str, dialect: Dialect, analysis: &ShellAnalysis) {
    let context = format!("{command:?} ({dialect:?})");
    for c in &analysis.commands {
        assert!(!c.words.is_empty(), "no words: {context}");
        assert!(!c.classes.is_empty(), "no classes: {context}");
        if c.classes.contains(&CommandClass::Unknown) {
            assert!(
                !c.targets_known,
                "unknown class with known targets: {context}"
            );
        }
        for (i, class) in c.classes.iter().enumerate() {
            assert!(
                !c.classes[..i].contains(class),
                "duplicate class: {context}"
            );
        }
        for t in &c.targets {
            assert!(!t.path.is_empty(), "empty target path: {context}");
        }
        if c.words[0] == "git" {
            assert!(c.git.is_some(), "git without facts: {context}");
        }
        if c.git.as_ref().is_some_and(|g| g.rewrites_history) {
            assert!(
                c.classes.contains(&CommandClass::GitHistoryRewrite),
                "rewrite without its class: {context}"
            );
        }
    }
}

#[test]
fn shape_holds_for_the_corpus_and_all_its_prefixes() {
    let cases = parse_corpus(CORPUS).unwrap_or_else(|e| panic!("{e}"));
    for case in &cases {
        let analysis = analyze(&case.command, case.dialect);
        check(&case.command, case.dialect, &analysis);
        // Every prefix, at character boundaries: truncated input must not panic or break the
        // contract.
        for (end, _) in case.command.char_indices() {
            let prefix = &case.command[..end];
            let analysis = analyze(prefix, case.dialect);
            check(prefix, case.dialect, &analysis);
        }
    }
}

#[test]
fn hostile_inputs_do_not_panic_or_overflow() {
    let depth = 5000;
    let inputs = [
        "(".repeat(depth),
        format!("{}rm x{}", "(".repeat(depth), ")".repeat(depth)),
        format!("{}rm x{}", "$(".repeat(depth), ")".repeat(depth)),
        format!("{}rm x{}", "{ ".repeat(depth), "; }".repeat(depth)),
        format!("{}x{}", "\"$(".repeat(depth), ")\"".repeat(depth)),
        "sh -c '".repeat(depth),
        format!("eval {}", "eval ".repeat(depth)),
        "a && ".repeat(depth) + "b",
        "a | ".repeat(depth) + "b",
        "if true; then ".repeat(depth),
        "\u{0}\u{1}\u{7f}rm \u{feff} \u{202e}x".to_string(),
        "rm \u{1F600} \"é\" 'ü'".to_string(),
        "x".repeat(1 << 20),
        " ".repeat(1 << 16) + "rm x",
        "\\".repeat(10_000),
        "'".repeat(10_001),
        "<<".repeat(1000),
    ];
    for dialect in [Dialect::Bash, Dialect::PowerShell, Dialect::Cmd] {
        for input in &inputs {
            let analysis = analyze(input, dialect);
            for c in &analysis.commands {
                assert!(!c.words.is_empty());
                assert!(!c.classes.is_empty());
            }
        }
    }
}

#[test]
fn nested_scripts_are_followed_a_few_levels_only() {
    // Each level quotes the next one; the innermost `rm` is found a few levels down but the
    // analysis does not recurse without bound.
    let mut script = "rm -rf deep".to_string();
    for _ in 0..12 {
        script = format!("sh -c '{}'", script.replace('\'', "'\\''"));
    }
    let analysis = analyze(&script, Dialect::Bash);
    assert!(analysis.has_unknown_targets());
    assert!(analysis.commands.len() >= 4);
    assert!(analysis.commands.len() <= 12);
}
