//! Recursive operations: what a directory reaches through links, and expansion of those reaches.
//!
//! The headline scenario is the 48,000-file incident: a scratch copy of the tree, outside the
//! repository, containing a junction (a symlink on Unix) into the live tree, and a recursive
//! delete aimed at the scratch copy.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::fs;

use acs_core::gate::PathLocation;
use common::{World, dir_link, files, in_repo, outside, touch};

const WALK_CAP: usize = 100_000;

/// `base/scratch/copy` with a file of its own and a link `live` to `target`.
fn scratch_copy_linked_to(w: &World, target: &std::path::Path) -> std::path::PathBuf {
    let copy = w.base.join("scratch").join("copy");
    touch(&copy.join("own.txt"));
    touch(&copy.join("nested").join("more.txt"));
    dir_link(target, &copy.join("live"));
    copy
}

#[test]
fn scratch_copy_with_a_junction_into_the_live_tree_reaches_the_repo() {
    let w = World::new();
    let r = w.resolver();
    let copy = scratch_copy_linked_to(&w, &w.repo.join("src"));

    // `rm -r scratch/copy`: the directory itself is outside the repository ...
    let target = r.resolve_recursive("scratch/copy", &w.base, WALK_CAP);
    assert_eq!(outside(&target), copy);
    // ... but the junction inside it reaches the live tree.
    let linked: Vec<&str> = target.linked_into_repo.iter().map(|p| p.as_str()).collect();
    assert_eq!(linked, ["src"]);

    // And the files a recursive delete would remove in the repository are listed.
    let expansion = r.expand_recursive(&target, 1_000);
    assert_eq!(files(&expansion), ["src/a.rs", "src/b.rs", "src/deep/c.rs"]);
    assert!(!expansion.truncated);

    // A delete of a path *through* the junction is a delete in the repository.
    let through = r.resolve("scratch/copy/live/a.rs", &w.base);
    assert_eq!(in_repo(&through), "src/a.rs");
    assert!(through.via_link);
    let through_dir = r.resolve_recursive("scratch/copy/live", &w.base, WALK_CAP);
    assert_eq!(in_repo(&through_dir), "src");
    assert!(through_dir.via_link);
    assert_eq!(
        files(&r.expand_recursive(&through_dir, 1_000)),
        files(&expansion)
    );
}

#[test]
fn a_junction_to_the_whole_live_tree_reaches_everything() {
    let w = World::new();
    let r = w.resolver();
    scratch_copy_linked_to(&w, &w.repo);
    let target = r.resolve_recursive("scratch/copy", &w.base, WALK_CAP);
    assert!(matches!(target.location, PathLocation::Outside(_)));
    let linked: Vec<&str> = target.linked_into_repo.iter().map(|p| p.as_str()).collect();
    assert_eq!(linked, ["."]);
    let expansion = r.expand_recursive(&target, 1_000);
    assert_eq!(
        files(&expansion),
        [
            ".git/HEAD",
            ".git/config",
            "README.md",
            "src/a.rs",
            "src/b.rs",
            "src/deep/c.rs"
        ]
    );
}

#[test]
fn a_scratch_copy_without_links_reaches_nothing() {
    let w = World::new();
    let r = w.resolver();
    let copy = w.base.join("scratch").join("copy");
    touch(&copy.join("a.txt"));
    touch(&copy.join("deeper").join("b.txt"));
    dir_link(&w.outside, &copy.join("elsewhere"));
    let target = r.resolve_recursive("scratch/copy", &w.base, WALK_CAP);
    assert!(target.linked_into_repo.is_empty());
    assert!(r.expand_recursive(&target, 100).files.is_empty());
}

#[test]
fn links_inside_a_repo_directory_are_reported_unless_they_stay_within_it() {
    let w = World::new();
    let r = w.resolver();
    touch(&w.repo.join("build").join("real").join("out.o"));
    dir_link(
        &w.repo.join("build").join("real"),
        &w.repo.join("build").join("inner"),
    );
    dir_link(&w.repo.join("src"), &w.repo.join("build").join("to-src"));
    let target = r.resolve_recursive("build", &w.repo, WALK_CAP);
    assert_eq!(in_repo(&target), "build");
    let linked: Vec<&str> = target.linked_into_repo.iter().map(|p| p.as_str()).collect();
    assert_eq!(linked, ["src"], "the link back into build/ adds nothing");

    let expansion = r.expand_recursive(&target, 1_000);
    assert_eq!(
        files(&expansion),
        [
            "build/inner",
            "build/real/out.o",
            "build/to-src",
            "src/a.rs",
            "src/b.rs",
            "src/deep/c.rs"
        ]
    );
}

#[test]
fn link_cycles_do_not_loop() {
    let w = World::new();
    let r = w.resolver();
    // Two repo directories linking to each other, and a link to its own parent.
    touch(&w.repo.join("a").join("fa.txt"));
    touch(&w.repo.join("b").join("fb.txt"));
    dir_link(&w.repo.join("b"), &w.repo.join("a").join("to-b"));
    dir_link(&w.repo.join("a"), &w.repo.join("b").join("to-a"));
    dir_link(&w.repo.join("a"), &w.repo.join("a").join("self"));
    let target = r.resolve_recursive("a", &w.repo, WALK_CAP);
    let linked: Vec<&str> = target.linked_into_repo.iter().map(|p| p.as_str()).collect();
    assert_eq!(linked, ["b"]);
    let expansion = r.expand_recursive(&target, 1_000);
    assert_eq!(
        files(&expansion),
        ["a/fa.txt", "a/self", "a/to-b", "b/fb.txt", "b/to-a"]
    );

    // The same from outside the repository.
    let copy = w.base.join("scratch");
    fs::create_dir_all(&copy).unwrap();
    dir_link(&copy, &copy.join("loop"));
    dir_link(&w.repo.join("b"), &copy.join("live"));
    let target = r.resolve_recursive("scratch", &w.base, WALK_CAP);
    let linked: Vec<&str> = target.linked_into_repo.iter().map(|p| p.as_str()).collect();
    assert_eq!(linked, ["b"]);
}

#[test]
fn nested_link_targets_collapse_into_the_outermost() {
    let w = World::new();
    let r = w.resolver();
    let copy = w.base.join("scratch").join("copy");
    fs::create_dir_all(&copy).unwrap();
    dir_link(&w.repo.join("src"), &copy.join("one"));
    dir_link(&w.repo.join("src").join("deep"), &copy.join("two"));
    let target = r.resolve_recursive("scratch/copy", &w.base, WALK_CAP);
    assert_eq!(target.linked_into_repo.len(), 2);
    let expansion = r.expand_recursive(&target, 1_000);
    assert_eq!(files(&expansion), ["src/a.rs", "src/b.rs", "src/deep/c.rs"]);
}

#[test]
fn the_walk_and_the_listing_are_capped() {
    let w = World::new();
    let r = w.resolver();
    for i in 0..50 {
        touch(&w.repo.join("many").join(format!("f{i:02}.txt")));
    }
    let capped = r.resolve_recursive_report("many", &w.repo, 10);
    assert!(capped.truncated);
    let whole = r.resolve_recursive_report("many", &w.repo, 1_000);
    assert!(!whole.truncated);

    let target = r.resolve("many", &w.repo);
    let listing = r.expand_recursive(&target, 20);
    assert_eq!(listing.files.len(), 20);
    assert!(listing.truncated);
    let all = r.expand_recursive(&target, 50);
    assert_eq!(all.files.len(), 50);
    assert!(!all.truncated, "exactly at the cap is not truncated");
}

#[test]
fn unresolvable_targets_expand_to_an_unknown_amount() {
    let w = World::new();
    let r = w.resolver();
    let target = r.resolve("", &w.repo);
    let expansion = r.expand_recursive(&target, 10);
    assert!(expansion.files.is_empty());
    assert!(expansion.truncated);
}

#[test]
fn a_plain_file_expands_to_itself() {
    let w = World::new();
    let r = w.resolver();
    let target = r.resolve("src/a.rs", &w.repo);
    assert_eq!(files(&r.expand_recursive(&target, 10)), ["src/a.rs"]);
    let missing = r.resolve("src/missing.rs", &w.repo);
    assert!(r.expand_recursive(&missing, 10).files.is_empty());
}

#[cfg(unix)]
mod unix_links {
    use super::*;
    use std::os::unix::fs::symlink;

    #[test]
    fn a_symlinked_file_in_the_scratch_copy_reaches_the_repo_file() {
        let w = World::new();
        let r = w.resolver();
        let copy = w.base.join("scratch").join("copy");
        touch(&copy.join("own.txt"));
        symlink(w.repo.join("README.md"), copy.join("readme-link")).unwrap();
        symlink(w.outside.join("file.txt"), copy.join("other-link")).unwrap();
        symlink(copy.join("missing"), copy.join("dangling")).unwrap();
        let target = r.resolve_recursive("scratch/copy", &w.base, WALK_CAP);
        let linked: Vec<&str> = target.linked_into_repo.iter().map(|p| p.as_str()).collect();
        assert_eq!(linked, ["README.md"]);
        assert_eq!(files(&r.expand_recursive(&target, 10)), ["README.md"]);
    }

    #[test]
    fn a_recursive_resolve_of_a_link_to_a_directory_follows_it() {
        let w = World::new();
        let r = w.resolver();
        symlink(w.repo.join("src"), w.base.join("shortcut")).unwrap();
        let target = r.resolve_recursive("shortcut", &w.base, WALK_CAP);
        assert_eq!(in_repo(&target), "src");
        assert!(target.via_link);
    }
}
