//! Prints the analysis of command lines: `analyze [--ps|--cmd] <file>`, where the file holds
//! commands separated by lines that consist of `%%`.
#![allow(missing_docs, clippy::unwrap_used)]

#[path = "../tests/common/mod.rs"]
mod common;

use acs_shell::{Dialect, analyze};

fn main() {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let dialect = match args.first().map(String::as_str) {
        Some("--ps") => Dialect::PowerShell,
        Some("--cmd") => Dialect::Cmd,
        _ => Dialect::Bash,
    };
    if args.first().is_some_and(|a| a.starts_with("--")) {
        args.remove(0);
    }
    let text = std::fs::read_to_string(&args[0]).unwrap();
    for command in text.split("\n%%\n") {
        let command = command.trim_end_matches('\n');
        let analysis = analyze(command, dialect);
        println!("{command}");
        println!("  complete={}", analysis.complete);
        for rendered in common::render(&analysis) {
            println!("  {}", serde_json::to_string(&rendered).unwrap());
        }
    }
}
