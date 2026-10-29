//! Timing of the hot path. Run with `cargo test -p acs-paths --release --test perf -- --ignored
//! --nocapture`.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::time::Instant;

use common::{World, in_repo};

#[test]
#[ignore = "timing measurement, run explicitly in release mode"]
fn resolving_an_existing_in_repo_file_is_fast() {
    let w = World::new();
    let r = w.resolver();
    let abs = w.repo.join("src").join("deep").join("c.rs");
    let abs = abs.to_str().unwrap();
    let rounds = 20_000u32;
    for (label, raw) in [("relative", "src/deep/c.rs"), ("absolute", abs)] {
        let start = Instant::now();
        for _ in 0..rounds {
            let resolved = r.resolve(std::hint::black_box(raw), &w.repo);
            assert_eq!(in_repo(&resolved), "src/deep/c.rs");
        }
        let per_call = start.elapsed() / rounds;
        println!("resolve ({label}): {per_call:?} per call");
        assert!(per_call.as_micros() < 200, "{per_call:?}");
    }
    let start = Instant::now();
    for _ in 0..200 {
        std::hint::black_box(acs_paths::PathResolver::new(&w.repo).unwrap());
    }
    println!("PathResolver::new: {:?} per call", start.elapsed() / 200);
}
