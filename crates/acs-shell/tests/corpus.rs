//! Table-driven test over `tests/fixtures/corpus.txt`: real-looking command lines with their
//! expected classifications and paths. Every mismatch is reported, not just the first.
//!
//! `ACS_SHELL_BLESS=1 cargo test -p acs-shell --test corpus` fills in the expectation of cases
//! that have none yet (never rewriting existing ones); review the diff before committing.

mod common;

use acs_shell::analyze;
use common::{CORPUS, Case, Rendered, is_case_start, parse_corpus, render};

fn show(commands: &[Rendered]) -> String {
    commands
        .iter()
        .map(|c| serde_json::to_string(c).unwrap_or_default())
        .collect::<Vec<_>>()
        .join("\n    ")
}

#[test]
fn corpus_matches_expectations() {
    let cases = parse_corpus(CORPUS).unwrap_or_else(|e| panic!("{e}"));
    assert!(
        cases.len() >= 200,
        "the corpus must have at least 200 commands, has {}",
        cases.len()
    );

    if std::env::var_os("ACS_SHELL_BLESS").is_some() {
        bless(&cases);
        return;
    }

    let mut failures = Vec::new();
    for case in &cases {
        if !case.blessed {
            failures.push(format!(
                "line {}: no expectation for {:?}",
                case.line, case.command
            ));
            continue;
        }
        let analysis = analyze(&case.command, case.dialect);
        let actual = render(&analysis);
        if actual != case.expected || analysis.complete != case.complete {
            failures.push(format!(
                "line {}: {:?} ({:?})\n  expected (complete={}):\n    {}\n  actual (complete={}):\n    {}",
                case.line,
                case.command,
                case.dialect,
                case.complete,
                show(&case.expected),
                analysis.complete,
                show(&actual),
            ));
        }
    }
    assert!(
        failures.is_empty(),
        "{} of {} cases differ:\n\n{}",
        failures.len(),
        cases.len(),
        failures.join("\n\n")
    );
}

/// Rewrites the corpus file, adding expectations to cases that have none.
fn bless(cases: &[Case]) {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/corpus.txt");
    let mut by_line = std::collections::HashMap::new();
    for case in cases.iter().filter(|c| !c.blessed) {
        let analysis = analyze(&case.command, case.dialect);
        let mut lines = Vec::new();
        if !analysis.complete {
            lines.push("> !incomplete".to_string());
        }
        if analysis.commands.is_empty() {
            lines.push("> (none)".to_string());
        }
        for rendered in render(&analysis) {
            lines.push(format!(
                "> {}",
                serde_json::to_string(&rendered).unwrap_or_default()
            ));
        }
        by_line.insert(case.line, lines);
    }
    // The new expectations go right after the last line of their case.
    let mut out = Vec::new();
    let mut pending: Option<usize> = None;
    let flush = |pending: &mut Option<usize>, out: &mut Vec<String>| {
        if let Some(line) = pending.take()
            && let Some(lines) = by_line.get(&line)
        {
            out.extend(lines.iter().cloned());
        }
    };
    for (index, line) in CORPUS.lines().enumerate() {
        if is_case_start(line) {
            flush(&mut pending, &mut out);
            pending = Some(index + 1);
        } else if !line.starts_with("| ") {
            flush(&mut pending, &mut out);
        }
        out.push(line.to_string());
    }
    flush(&mut pending, &mut out);
    let mut text = out.join("\n");
    text.push('\n');
    std::fs::write(path, text).unwrap_or_else(|e| panic!("cannot write the corpus: {e}"));
}
