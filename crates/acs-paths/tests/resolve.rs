//! Resolution of single paths: classification, `..`, links, loops, `.git`, case.

mod common;

use std::fs;
use std::path::Path;

use acs_core::gate::PathLocation;
use acs_paths::PathResolver;
use common::{World, dir_link, in_repo, outside, touch};

#[test]
fn absolute_relative_and_dot_segments_agree() {
    let w = World::new();
    let r = w.resolver();
    let abs = w.repo.join("src").join("a.rs");
    let spellings = [
        (abs.to_string_lossy().into_owned(), w.base.clone()),
        ("src/a.rs".to_owned(), w.repo.clone()),
        ("./src/./a.rs".to_owned(), w.repo.clone()),
        ("src/deep/../a.rs".to_owned(), w.repo.clone()),
        ("../a.rs".to_owned(), w.repo.join("src").join("deep")),
        ("a.rs".to_owned(), w.repo.join("src")),
        (
            w.repo
                .join("src")
                .join("..")
                .join("src")
                .join("a.rs")
                .to_string_lossy()
                .into_owned(),
            w.outside.clone(),
        ),
    ];
    for (raw, cwd) in spellings {
        let resolved = r.resolve(&raw, &cwd);
        assert_eq!(in_repo(&resolved), "src/a.rs", "{raw:?} from {cwd:?}");
        assert!(!resolved.via_link, "{raw:?}");
        assert_eq!(resolved.raw, raw);
        assert!(resolved.linked_into_repo.is_empty());
    }
}

#[test]
fn root_and_trailing_separators() {
    let w = World::new();
    let r = w.resolver();
    assert_eq!(in_repo(&r.resolve(".", &w.repo)), ".");
    assert_eq!(
        in_repo(&r.resolve(w.repo.to_str().unwrap(), &w.outside)),
        "."
    );
    assert_eq!(in_repo(&r.resolve("src/", &w.repo)), "src");
    assert_eq!(in_repo(&r.resolve("src//deep///", &w.repo)), "src/deep");
    assert_eq!(in_repo(&r.resolve("src/..", &w.repo)), ".");
}

#[test]
fn paths_that_do_not_exist_yet() {
    let w = World::new();
    let r = w.resolver();
    assert_eq!(in_repo(&r.resolve("src/new.rs", &w.repo)), "src/new.rs");
    assert_eq!(in_repo(&r.resolve("a/b/c/new.rs", &w.repo)), "a/b/c/new.rs");
    assert_eq!(in_repo(&r.resolve("a/b/../c.rs", &w.repo)), "a/c.rs");
    assert_eq!(in_repo(&r.resolve("nope/../src/a.rs", &w.repo)), "src/a.rs");
    // A file used as a directory: the rest cannot exist, but the path is still the repo's.
    assert_eq!(in_repo(&r.resolve("README.md/x", &w.repo)), "README.md/x");
    let beyond = r.resolve("../new/dir/file", &w.repo);
    assert_eq!(
        outside(&beyond),
        w.base.join("new").join("dir").join("file")
    );
}

#[test]
fn leaving_the_repository() {
    let w = World::new();
    let r = w.resolver();
    let up = r.resolve("../outside/file.txt", &w.repo);
    assert_eq!(outside(&up), w.outside.join("file.txt"));
    assert!(!up.via_link);
    let abs = r.resolve(w.outside.join("file.txt").to_str().unwrap(), &w.repo);
    assert_eq!(outside(&abs), w.outside.join("file.txt"));
    // A sibling whose name merely starts with the repo's name is not inside it.
    let sibling = w.base.join("repo-two").join("x");
    assert_eq!(
        outside(&r.resolve(sibling.to_str().unwrap(), &w.repo)),
        sibling
    );
    // `..` past the filesystem root stays at the root.
    let deep = "../".repeat(200) + "etc/passwd";
    assert!(matches!(
        r.resolve(&deep, &w.repo).location,
        PathLocation::Outside(_)
    ));
}

#[test]
fn dot_dot_after_a_link_follows_the_link() {
    let w = World::new();
    let r = w.resolver();
    // repo/lnk -> repo/src/deep, so `lnk/..` is repo/src, not repo.
    dir_link(&w.repo.join("src").join("deep"), &w.repo.join("lnk"));
    let resolved = r.resolve("lnk/../a.rs", &w.repo);
    assert_eq!(in_repo(&resolved), "src/a.rs");
    assert!(resolved.via_link);

    // A link out of the repository: `ext/..` is the outside directory's parent.
    dir_link(&w.outside, &w.repo.join("ext"));
    let resolved = r.resolve("ext/../repo/README.md", &w.repo);
    assert_eq!(in_repo(&resolved), "README.md");
    assert!(resolved.via_link);
}

#[test]
fn link_inside_the_repo_pointing_outside_is_outside() {
    let w = World::new();
    let r = w.resolver();
    dir_link(&w.outside, &w.repo.join("ext"));
    let resolved = r.resolve("ext/file.txt", &w.repo);
    assert_eq!(outside(&resolved), w.outside.join("file.txt"));
    assert!(resolved.via_link);
    // Creating a new file through it lands outside too.
    let created = r.resolve("ext/new/created.txt", &w.repo);
    assert_eq!(outside(&created), w.outside.join("new").join("created.txt"));
    assert!(created.via_link);
}

#[test]
fn link_outside_pointing_into_the_repo_is_in_the_repo() {
    let w = World::new();
    let r = w.resolver();
    dir_link(&w.repo.join("src"), &w.outside.join("live"));
    let resolved = r.resolve("live/a.rs", &w.outside);
    assert_eq!(in_repo(&resolved), "src/a.rs");
    assert!(resolved.via_link);
    // A link straight to the root counts as a link too.
    dir_link(&w.repo, &w.outside.join("whole"));
    let resolved = r.resolve("whole/src/b.rs", &w.outside);
    assert_eq!(in_repo(&resolved), "src/b.rs");
    assert!(resolved.via_link);
}

#[test]
fn a_link_naming_an_ancestor_of_the_root_is_not_via_link() {
    // The macOS tempdir shape: `/var` -> `/private/var`. The repository lives below the link.
    let w = World::new();
    let holder = tempfile::tempdir().unwrap();
    let alias = dunce::canonicalize(holder.path()).unwrap().join("up");
    dir_link(&w.base, &alias);
    let r = w.resolver();
    let spelled = alias.join("repo").join("src").join("a.rs");
    let resolved = r.resolve(spelled.to_str().unwrap(), &alias);
    assert_eq!(in_repo(&resolved), "src/a.rs");
    assert!(
        !resolved.via_link,
        "an ancestor link is not a way out of the repo"
    );
    // The same file relative to a cwd spelled through the alias.
    let resolved = r.resolve("src/a.rs", &alias.join("repo"));
    assert_eq!(in_repo(&resolved), "src/a.rs");
    assert!(!resolved.via_link);
}

#[test]
fn git_paths_are_repo_paths() {
    let w = World::new();
    let r = w.resolver();
    assert_eq!(in_repo(&r.resolve(".git/config", &w.repo)), ".git/config");
    assert_eq!(
        in_repo(&r.resolve("src/../.git/hooks/pre-commit", &w.repo)),
        ".git/hooks/pre-commit"
    );
    assert_eq!(in_repo(&r.resolve(".git", &w.repo)), ".git");
    assert_eq!(r.git_dir(), Some(w.repo.join(".git").as_path()));
}

#[test]
fn linked_worktree_git_dir_is_mapped_to_dot_git() {
    let w = World::new();
    // A linked worktree: `.git` is a file pointing into the main repo's `.git/worktrees/wt`.
    let main_git = w.repo.join(".git");
    let wt_git = main_git.join("worktrees").join("wt");
    touch(&wt_git.join("HEAD"));
    fs::write(wt_git.join("commondir"), "../..\n").unwrap();
    let wt = w.base.join("wt");
    fs::create_dir_all(&wt).unwrap();
    fs::write(wt.join(".git"), format!("gitdir: {}\n", wt_git.display())).unwrap();
    touch(&wt.join("src").join("x.rs"));

    let r = PathResolver::new(&wt).unwrap();
    assert_eq!(r.git_dir(), Some(wt_git.as_path()));
    assert_eq!(in_repo(&r.resolve("src/x.rs", &wt)), "src/x.rs");
    assert_eq!(in_repo(&r.resolve(".git", &wt)), ".git");
    // Paths in the real git directory are `.git/...`, so sealed globs for `.git/**` catch them.
    let head = r.resolve(wt_git.join("HEAD").to_str().unwrap(), &wt);
    assert_eq!(in_repo(&head), ".git/HEAD");
    let hooks = r.resolve(
        main_git.join("hooks").join("pre-commit").to_str().unwrap(),
        &wt,
    );
    assert_eq!(in_repo(&hooks), ".git/hooks/pre-commit");
    // The main working tree is a different place.
    assert_eq!(
        outside(&r.resolve(w.repo.join("README.md").to_str().unwrap(), &wt)),
        w.repo.join("README.md")
    );
}

#[test]
fn unusable_input_is_unresolvable_not_a_panic() {
    let w = World::new();
    let r = w.resolver();
    for raw in ["", "a\0b"] {
        assert!(
            matches!(
                r.resolve(raw, &w.repo).location,
                PathLocation::Unresolvable(_)
            ),
            "{raw:?}"
        );
    }
    let relative_cwd = r.resolve("src/a.rs", Path::new("relative/dir"));
    match relative_cwd.location {
        PathLocation::Unresolvable(reason) => assert!(reason.contains("absolute"), "{reason}"),
        other => panic!("{other:?}"),
    }
}

#[test]
fn building_a_resolver_needs_a_real_directory() {
    let w = World::new();
    assert!(PathResolver::new(&w.base.join("missing")).is_err());
    assert!(PathResolver::new(&w.repo.join("README.md")).is_err());
}

#[test]
fn case_is_fixed_to_the_on_disk_spelling_when_insensitive() {
    // The flag is injected, so this exercises the case-fixing on every platform: with it set,
    // lookups ignore case and the on-disk name wins.
    let w = World::new();
    let r = w.resolver().with_case_insensitive(true);
    assert!(r.case_insensitive());
    assert_eq!(in_repo(&r.resolve("SRC/A.RS", &w.repo)), "src/a.rs");
    assert_eq!(
        in_repo(&r.resolve("Src/Deep/C.rs", &w.repo)),
        "src/deep/c.rs"
    );
    assert_eq!(in_repo(&r.resolve("readme.MD", &w.repo)), "README.md");
    // Not-yet-existing tails keep the spelling given; the existing prefix is fixed.
    assert_eq!(in_repo(&r.resolve("SRC/New.rs", &w.repo)), "src/New.rs");
    // The repository root itself may be spelled in another case.
    let shouting = w.repo.to_str().unwrap().to_uppercase();
    assert_eq!(
        in_repo(&r.resolve(&format!("{shouting}/src/a.rs"), &w.outside)),
        "src/a.rs"
    );
    // .git, case-insensitively.
    assert_eq!(in_repo(&r.resolve(".GIT/CONFIG", &w.repo)), ".git/config");
}

#[cfg(target_os = "linux")]
#[test]
fn case_is_preserved_when_sensitive() {
    let w = World::new();
    let r = w.resolver();
    assert!(
        !r.case_insensitive(),
        "temp dirs on Linux are case-sensitive"
    );
    // `SRC/a.rs` is a different, nonexistent path here.
    assert_eq!(in_repo(&r.resolve("SRC/a.rs", &w.repo)), "SRC/a.rs");
}

#[cfg(any(windows, target_os = "macos"))]
#[test]
fn real_case_insensitive_filesystems_report_on_disk_case() {
    let w = World::new();
    let r = w.resolver();
    assert!(
        r.case_insensitive(),
        "CI's macOS and Windows temp dirs ignore case"
    );
    assert_eq!(in_repo(&r.resolve("SRC/A.RS", &w.repo)), "src/a.rs");
    assert_eq!(in_repo(&r.resolve("readme.md", &w.repo)), "README.md");
    assert_eq!(in_repo(&r.resolve("Src/Deep/../B.rs", &w.repo)), "src/b.rs");
    let shouting = w.repo.to_str().unwrap().to_uppercase();
    assert_eq!(
        in_repo(&r.resolve(&format!("{shouting}/src/a.rs"), &w.outside)),
        "src/a.rs"
    );
}

#[cfg(target_os = "macos")]
#[test]
fn macos_tempdir_private_prefix_does_not_break_classification() {
    // `tempfile` reports `/var/folders/...`, which is a link to `/private/var/folders/...`.
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("repo");
    touch(&repo.join("src").join("a.rs"));
    let r = PathResolver::new(&repo).unwrap();
    assert!(r.root().starts_with("/private"), "{:?}", r.root());
    let spelled = repo.join("src").join("a.rs");
    assert_eq!(
        in_repo(&r.resolve(spelled.to_str().unwrap(), &repo)),
        "src/a.rs"
    );
    assert_eq!(in_repo(&r.resolve("src/a.rs", &repo)), "src/a.rs");
    assert!(!r.resolve(spelled.to_str().unwrap(), &repo).via_link);
}

#[cfg(unix)]
mod unix_links {
    use super::*;
    use std::os::unix::fs::symlink;

    #[test]
    fn file_links_are_followed_by_resolve_but_not_by_resolve_entry() {
        let w = World::new();
        let r = w.resolver();
        symlink(w.outside.join("file.txt"), w.repo.join("escape.txt")).unwrap();
        symlink("src/a.rs", w.repo.join("alias.rs")).unwrap();

        let followed = r.resolve("escape.txt", &w.repo);
        assert_eq!(outside(&followed), w.outside.join("file.txt"));
        assert!(followed.via_link);

        // `rm escape.txt` removes the link, which is in the repo.
        let entry = r.resolve_entry("escape.txt", &w.repo);
        assert_eq!(in_repo(&entry), "escape.txt");
        assert!(!entry.via_link);
        // With a trailing separator the link is followed (POSIX).
        let slash = r.resolve_entry("alias.rs/", &w.repo);
        assert_eq!(in_repo(&slash), "src/a.rs");
        assert!(slash.via_link);

        assert_eq!(in_repo(&r.resolve("alias.rs", &w.repo)), "src/a.rs");
        assert_eq!(in_repo(&r.resolve_entry("alias.rs", &w.repo)), "alias.rs");
        // Links in leading components are followed even for entries.
        dir_link(&w.outside, &w.repo.join("ext"));
        assert_eq!(
            outside(&r.resolve_entry("ext/file.txt", &w.repo)),
            w.outside.join("file.txt")
        );
    }

    #[test]
    fn relative_link_targets_resolve_against_the_links_directory() {
        let w = World::new();
        let r = w.resolver();
        symlink("../src/a.rs", w.repo.join("src").join("deep").join("up.rs")).unwrap();
        let resolved = r.resolve("src/deep/up.rs", &w.repo);
        // `src/deep/../src/a.rs` does not exist, so the target is a new path under src.
        assert_eq!(in_repo(&resolved), "src/src/a.rs");
        symlink("../a.rs", w.repo.join("src").join("deep").join("ok.rs")).unwrap();
        assert_eq!(in_repo(&r.resolve("src/deep/ok.rs", &w.repo)), "src/a.rs");
    }

    #[test]
    fn link_loops_are_unresolvable() {
        let w = World::new();
        let r = w.resolver();
        symlink("b", w.repo.join("a")).unwrap();
        symlink("a", w.repo.join("b")).unwrap();
        symlink("self", w.repo.join("self")).unwrap();
        for raw in ["a", "b/x", "self", "self/deeper/file"] {
            match r.resolve(raw, &w.repo).location {
                PathLocation::Unresolvable(reason) => assert!(reason.contains("links"), "{reason}"),
                other => panic!("{raw}: {other:?}"),
            }
        }
        // `rm a` acts on the link itself, which is fine.
        assert_eq!(in_repo(&r.resolve_entry("a", &w.repo)), "a");
    }

    #[test]
    fn dangling_links_resolve_to_where_a_write_would_land() {
        let w = World::new();
        let r = w.resolver();
        symlink("generated/out.txt", w.repo.join("dangling")).unwrap();
        let resolved = r.resolve("dangling", &w.repo);
        assert_eq!(in_repo(&resolved), "generated/out.txt");
        assert!(resolved.via_link);
        symlink(w.outside.join("not-yet"), w.repo.join("dangling-out")).unwrap();
        assert_eq!(
            outside(&r.resolve("dangling-out", &w.repo)),
            w.outside.join("not-yet")
        );
    }

    #[test]
    fn a_dot_git_symlink_to_elsewhere_is_outside() {
        let w = World::new();
        // `.git` replaced by a link to a directory elsewhere is a "separate git dir".
        let elsewhere = w.base.join("gitdir");
        fs::rename(w.repo.join(".git"), &elsewhere).unwrap();
        symlink(&elsewhere, w.repo.join(".git")).unwrap();
        let r = w.resolver();
        assert_eq!(in_repo(&r.resolve(".git/config", &w.repo)), ".git/config");
        assert_eq!(
            in_repo(&r.resolve(elsewhere.join("HEAD").to_str().unwrap(), &w.repo)),
            ".git/HEAD"
        );
    }
}
