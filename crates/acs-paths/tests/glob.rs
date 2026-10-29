//! Glob expansion with shell semantics.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::path::Path;

use common::{World, dir_link, files, touch};

/// The standard world plus hidden files, an ignored file and a nested directory.
fn world() -> World {
    let w = World::new();
    for file in [
        ".env",
        ".gitignore",
        "ignored.txt",
        "notes.txt",
        ".hidden/secret.rs",
        "src/.dot.rs",
        "src/deep/d.rs",
    ] {
        touch(&w.repo.join(file));
    }
    std::fs::write(w.repo.join(".gitignore"), "ignored.txt\n").unwrap();
    w
}

fn expand(w: &World, pattern: &str, recursive: bool) -> Vec<String> {
    let expansion = w.resolver().expand_glob(pattern, &w.repo, recursive, 1_000);
    assert!(!expansion.truncated, "{pattern}");
    files(&expansion).into_iter().map(str::to_owned).collect()
}

#[test]
fn star_does_not_cross_separators_or_match_leading_dots() {
    let w = world();
    assert_eq!(expand(&w, "src/*.rs", false), ["src/a.rs", "src/b.rs"]);
    assert_eq!(expand(&w, "*.rs", false), Vec::<String>::new());
    // `*` at the root: regular files only (no directories without -r), no dotfiles.
    assert_eq!(
        expand(&w, "*", false),
        ["README.md", "ignored.txt", "notes.txt"]
    );
    // A pattern that starts with a dot matches dotfiles.
    assert_eq!(expand(&w, ".*", false), [".env", ".gitignore"]);
    assert_eq!(expand(&w, "src/.*.rs", false), ["src/.dot.rs"]);
}

#[test]
fn question_mark_and_brackets() {
    let w = world();
    assert_eq!(expand(&w, "src/?.rs", false), ["src/a.rs", "src/b.rs"]);
    assert_eq!(expand(&w, "src/[a].rs", false), ["src/a.rs"]);
    assert_eq!(expand(&w, "src/[!a].rs", false), ["src/b.rs"]);
    assert_eq!(expand(&w, "src/[ab].rs", false), ["src/a.rs", "src/b.rs"]);
    // An unterminated bracket is a literal, as in the shell.
    assert!(expand(&w, "src/[a.rs", false).is_empty());
}

#[test]
fn double_star_matches_directories_at_any_depth() {
    let w = world();
    assert_eq!(
        expand(&w, "src/**/*.rs", false),
        ["src/a.rs", "src/b.rs", "src/deep/c.rs", "src/deep/d.rs"]
    );
    assert_eq!(expand(&w, "**/c.rs", false), ["src/deep/c.rs"]);
    // Last, it matches everything beneath.
    assert_eq!(
        expand(&w, "src/deep/**", false),
        ["src/deep/c.rs", "src/deep/d.rs"]
    );
}

#[test]
fn gitignore_is_not_consulted() {
    let w = world();
    assert_eq!(expand(&w, "*.txt", false), ["ignored.txt", "notes.txt"]);
}

#[test]
fn recursive_expansion_lists_the_files_under_matched_directories() {
    let w = world();
    assert_eq!(
        expand(&w, "src", true),
        [
            "src/.dot.rs",
            "src/a.rs",
            "src/b.rs",
            "src/deep/c.rs",
            "src/deep/d.rs"
        ]
    );
    assert_eq!(
        expand(&w, "s*/de*", true),
        ["src/deep/c.rs", "src/deep/d.rs"]
    );
    // `rm -r *` does not reach hidden entries, but a literal `.hidden` does, and `.` is the
    // whole tree including `.git`.
    let star = expand(&w, "*", true);
    assert!(star.contains(&"src/a.rs".to_owned()));
    assert!(
        !star
            .iter()
            .any(|f| f.starts_with(".hidden") || f.starts_with(".git/"))
    );
    assert_eq!(expand(&w, ".hidden", true), [".hidden/secret.rs"]);
    let everything = expand(&w, ".", true);
    assert!(everything.contains(&".git/config".to_owned()));
    assert!(everything.contains(&".hidden/secret.rs".to_owned()));
}

#[test]
fn trailing_separator_matches_directories_only() {
    let w = world();
    assert!(
        expand(&w, "*/", false).is_empty(),
        "directories need -r to be deleted"
    );
    assert_eq!(
        expand(&w, "src/*/", true),
        ["src/deep/c.rs", "src/deep/d.rs"]
    );
    assert!(expand(&w, "README.md/", true).is_empty());
}

#[test]
fn plain_paths_work_and_missing_ones_expand_to_nothing() {
    let w = world();
    assert_eq!(expand(&w, "src/a.rs", false), ["src/a.rs"]);
    assert!(expand(&w, "src/missing.rs", false).is_empty());
    assert!(expand(&w, "nodir/*.rs", true).is_empty());
    // `..` and absolute patterns.
    let r = w.resolver();
    let abs = w.repo.join("src").join("*.rs");
    let expansion = r.expand_glob(abs.to_str().unwrap(), &w.outside, false, 100);
    assert_eq!(files(&expansion), ["src/a.rs", "src/b.rs"]);
    let up = r.expand_glob("../a*.rs", &w.repo.join("src").join("deep"), false, 100);
    assert_eq!(files(&up), ["src/a.rs"]);
}

#[test]
fn patterns_outside_the_repo_reach_nothing_in_it() {
    let w = world();
    let r = w.resolver();
    let expansion = r.expand_glob("*", &w.outside, true, 100);
    assert!(expansion.files.is_empty());
    assert!(!expansion.truncated);
}

#[test]
fn a_glob_over_a_scratch_copy_finds_the_live_tree_through_the_junction() {
    let w = world();
    let copy = w.base.join("scratch").join("copy");
    touch(&copy.join("own.txt"));
    dir_link(&w.repo.join("src").join("deep"), &copy.join("live"));
    let r = w.resolver();
    let expansion = r.expand_glob("copy/*", &w.base.join("scratch"), true, 100);
    assert_eq!(files(&expansion), ["src/deep/c.rs", "src/deep/d.rs"]);
    // Without -r nothing is deleted through the junction itself.
    let shallow = r.expand_glob("copy/*", &w.base.join("scratch"), false, 100);
    assert!(shallow.files.is_empty());
}

#[test]
fn the_listing_is_capped() {
    let w = world();
    let r = w.resolver();
    let expansion = r.expand_glob("src/**", &w.repo, true, 2);
    assert_eq!(expansion.files.len(), 2);
    assert!(expansion.truncated);
    let exact = r.expand_glob("src/*.rs", &w.repo, false, 2);
    assert_eq!(exact.files.len(), 2);
    assert!(!exact.truncated);
}

#[test]
fn case_insensitive_filesystems_match_case_insensitively() {
    let w = world();
    let r = w.resolver().with_case_insensitive(true);
    let expansion = r.expand_glob("SRC/*.RS", &w.repo, false, 100);
    assert_eq!(files(&expansion), ["src/a.rs", "src/b.rs"]);
    let sensitive = w.resolver().with_case_insensitive(false);
    assert!(
        sensitive
            .expand_glob("src/*.RS", &w.repo, false, 100)
            .files
            .is_empty()
    );
}

#[test]
fn relative_cwd_is_reported_as_unknown() {
    let w = world();
    let expansion = w
        .resolver()
        .expand_glob("*", Path::new("relative"), true, 10);
    assert!(expansion.files.is_empty());
    assert!(expansion.truncated);
}

#[cfg(unix)]
#[test]
fn a_matched_link_is_one_entry() {
    use std::os::unix::fs::symlink;
    let w = world();
    symlink(&w.outside, w.repo.join("ext")).unwrap();
    symlink("src/a.rs", w.repo.join("alias.rs")).unwrap();
    assert_eq!(expand(&w, "ext", false), ["ext"]);
    // Recursively, the link is still just the link: the outside tree is not in the repository.
    assert_eq!(expand(&w, "ext", true), ["ext"]);
    assert_eq!(expand(&w, "*.rs", false), ["alias.rs"]);
}
