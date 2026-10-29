//! Keeps `spec/SPEC.md` honest: the tables it contains must agree with the code.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::fs;
use std::path::PathBuf;

use acs_core::{DEFAULT_SEALED, Guards, Outcome, ReasonCode, pilot_id};

fn spec() -> String {
    fs::read_to_string(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../spec/SPEC.md"))
        .unwrap()
}

/// Table rows of the form ``| `a` | b | ...``, as the cells of each row, in document order.
fn rows_starting_with_code(spec: &str) -> Vec<Vec<String>> {
    spec.lines()
        .filter(|line| line.starts_with("| `"))
        .map(|line| {
            line.trim_matches('|')
                .split('|')
                .map(|cell| cell.trim().to_owned())
                .collect()
        })
        .collect()
}

#[test]
fn reason_code_table_matches_the_code() {
    let spec = spec();
    let rows = rows_starting_with_code(&spec);
    for code in ReasonCode::ALL {
        let cell = format!("`{code}`");
        let row = rows
            .iter()
            .find(|row| row[0] == cell && row.len() == 3)
            .unwrap_or_else(|| panic!("SPEC.md has no reason-code table row for {code}"));
        let wanted = match code.typical_outcomes()[0] {
            Outcome::Allow => "allow",
            Outcome::Deny => "deny",
            Outcome::Ask => "ask",
            Outcome::Queue => "queue",
        };
        assert!(
            row[1].starts_with(wanted),
            "{code}: SPEC.md says {:?}, code says {wanted}",
            row[1]
        );
    }
}

#[test]
fn default_sealed_list_matches_the_code_and_the_example() {
    let spec = spec();
    let after = spec
        .split("**Default sealed globs.**")
        .nth(1)
        .expect("SPEC.md lacks the default sealed globs");
    let block: Vec<&str> = after
        .split("```")
        .nth(1)
        .unwrap()
        .lines()
        .skip(1)
        .filter(|l| !l.is_empty())
        .collect();
    assert_eq!(block, DEFAULT_SEALED);

    let example = fs::read_to_string(
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../spec/examples/guards.toml"),
    )
    .unwrap();
    let guards: Guards = toml::from_str(&example).unwrap();
    assert_eq!(guards.sealed, DEFAULT_SEALED);
}

#[test]
fn pilot_id_table_matches_the_code() {
    let spec = spec();
    for row in rows_starting_with_code(&spec)
        .iter()
        .filter(|row| row[0].contains('@'))
    {
        let email = row[0].trim_matches('`');
        let expected = row[1].trim_matches('`');
        assert_eq!(pilot_id(email), expected, "{email}");
    }
}
