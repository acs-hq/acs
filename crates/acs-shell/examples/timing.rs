//! Measures `analyze` on typical command lines: `cargo run --release -p acs-shell --example timing`.
#![allow(missing_docs, clippy::unwrap_used, clippy::print_stdout)]

use std::hint::black_box;
use std::time::Instant;

use acs_shell::{Dialect, analyze};

fn per_call(command: &str, dialect: Dialect, iterations: u32) -> f64 {
    for _ in 0..200 {
        black_box(analyze(black_box(command), dialect));
    }
    let start = Instant::now();
    for _ in 0..iterations {
        black_box(analyze(black_box(command), dialect));
    }
    start.elapsed().as_secs_f64() * 1e6 / f64::from(iterations)
}

fn main() {
    let commands = [
        ("trivial", "ls"),
        ("typical", "cd sub && rm -rf build"),
        ("git push", "git push --force-with-lease origin main"),
        ("find", "find . -name '*.tmp' -delete"),
        (
            "pipeline",
            "git diff --name-only | xargs sed -i 's/a/b/' && cargo fmt --all && cargo test -p acs-core",
        ),
        (
            "long",
            "for f in $(ls src/*.rs); do cp \"$f\" \"/tmp/backup/$f\" && sed -i 's/foo/bar/g' \"$f\"; done; \
             git add -A && git commit -m 'fix: rename things' && git push origin HEAD",
        ),
    ];
    for (name, command) in commands {
        println!(
            "{name:10} {:8.1} us  ({} bytes)",
            per_call(command, Dialect::Bash, 20_000),
            command.len()
        );
    }
    println!(
        "powershell {:8.1} us",
        per_call(
            "Remove-Item -Recurse -Force build; Get-ChildItem *.log | Remove-Item",
            Dialect::PowerShell,
            20_000
        )
    );
    println!(
        "cmd        {:8.1} us",
        per_call(
            "cd build && del /s /q *.tmp & rd /s /q out",
            Dialect::Cmd,
            20_000
        )
    );

    // The cost of creating the parser alone.
    let iterations = 20_000u32;
    let start = Instant::now();
    for _ in 0..iterations {
        let mut parser = tree_sitter::Parser::new();
        parser
            .set_language(&tree_sitter_bash::LANGUAGE.into())
            .unwrap();
        black_box(&parser);
    }
    println!(
        "parser creation (Parser::new + set_language): {:.2} us",
        start.elapsed().as_secs_f64() * 1e6 / f64::from(iterations)
    );
}
