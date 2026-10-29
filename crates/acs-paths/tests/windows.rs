//! Windows path shapes: verbatim and UNC prefixes, drive-letter case, separators, junctions.
//!
//! The whole file is Windows-only; CI runs it on the Windows runner.

#![cfg(windows)]
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::path::PathBuf;

use acs_core::gate::PathLocation;
use common::{World, dir_link, files, in_repo, outside, touch};

#[test]
fn verbatim_prefix_is_stripped() {
    let w = World::new();
    let r = w.resolver();
    let verbatim = format!(r"\\?\{}", w.repo.join("src").join("a.rs").display());
    assert_eq!(in_repo(&r.resolve(&verbatim, &w.outside)), "src/a.rs");
    let verbatim_out = format!(r"\\?\{}", w.outside.join("file.txt").display());
    assert_eq!(
        outside(&r.resolve(&verbatim_out, &w.repo)),
        w.outside.join("file.txt")
    );
    // A verbatim cwd works the same way.
    let cwd = PathBuf::from(format!(r"\\?\{}", w.repo.display()));
    assert_eq!(in_repo(&r.resolve(r"src\a.rs", &cwd)), "src/a.rs");
}

#[test]
fn drive_letter_case_and_separators_do_not_matter() {
    let w = World::new();
    let r = w.resolver();
    let abs = w.repo.join("src").join("a.rs").display().to_string();
    let mut lower = abs.clone();
    lower[..1].make_ascii_lowercase();
    assert_eq!(in_repo(&r.resolve(&lower, &w.outside)), "src/a.rs");
    let mut upper = abs.clone();
    upper[..1].make_ascii_uppercase();
    assert_eq!(in_repo(&r.resolve(&upper, &w.outside)), "src/a.rs");
    let mixed = abs.replace('\\', "/");
    assert_eq!(in_repo(&r.resolve(&mixed, &w.outside)), "src/a.rs");
    assert_eq!(
        in_repo(&r.resolve(r"src/deep\..\a.rs", &w.repo)),
        "src/a.rs"
    );
    assert_eq!(in_repo(&r.resolve("src\\", &w.repo)), "src");
}

#[test]
fn rooted_and_drive_relative_paths_use_the_cwd_drive() {
    let w = World::new();
    let r = w.resolver();
    let abs = w.repo.join("src").join("a.rs").display().to_string();
    let without_drive = &abs[2..];
    assert_eq!(in_repo(&r.resolve(without_drive, &w.outside)), "src/a.rs");
    let drive = &abs[..2];
    assert_eq!(
        in_repo(&r.resolve(&format!(r"{drive}src\a.rs"), &w.repo)),
        "src/a.rs"
    );
    // Another drive's relative path cannot be resolved.
    let other = if drive.eq_ignore_ascii_case("Z:") {
        "Y:"
    } else {
        "Z:"
    };
    assert!(matches!(
        r.resolve(&format!(r"{other}file"), &w.repo).location,
        PathLocation::Unresolvable(_)
    ));
}

#[test]
fn trailing_dots_spaces_and_streams_name_the_same_file() {
    let w = World::new();
    let r = w.resolver();
    for raw in [
        r"src\a.rs.",
        r"src\a.rs ",
        r"src\a.rs::$DATA",
        r"src\a.rs:stream",
    ] {
        assert_eq!(in_repo(&r.resolve(raw, &w.repo)), "src/a.rs", "{raw}");
    }
    // Unix tools may also see the file under the other case.
    assert_eq!(in_repo(&r.resolve(r"SRC\A.RS", &w.repo)), "src/a.rs");
}

#[test]
fn device_names_are_not_repository_files() {
    let w = World::new();
    let r = w.resolver();
    for raw in ["NUL", "nul", r"src\con", "COM1"] {
        match r.resolve(raw, &w.repo).location {
            PathLocation::Outside(device) => assert!(device.starts_with(r"\\.\"), "{device}"),
            other => panic!("{raw}: {other:?}"),
        }
    }
}

#[test]
fn unc_paths_are_resolved_without_panicking() {
    let w = World::new();
    let r = w.resolver();
    // The administrative share of the local drive names the same files. Whether Windows maps it
    // back to the local path or leaves it a network path (outside, or unresolvable when access
    // is denied), resolving it must not panic.
    let drive = w.repo.display().to_string();
    let unc = format!(r"\\localhost\{}${}", &drive[..1], &drive[2..]);
    let _ = r.resolve(&format!(r"{unc}\src\a.rs"), &w.outside);
    // A server/share that does not exist is just a path outside the repository.
    let missing = r.resolve(r"\\acs-no-such-server\share\dir\file.txt", &w.repo);
    assert!(matches!(
        missing.location,
        PathLocation::Outside(_) | PathLocation::Unresolvable(_)
    ));
}

#[test]
fn junction_scratch_copy_is_classified_as_reaching_the_live_tree() {
    // The shape of the 48,000-file incident: a scratch copy outside the repository holding a
    // junction (`mklink /J`, no admin rights needed) into the live tree.
    let w = World::new();
    let r = w.resolver();
    let copy = w.base.join("scratch").join("copy");
    touch(&copy.join("own.txt"));
    dir_link(&w.repo.join("src"), &copy.join("live"));

    let target = r.resolve_recursive(copy.to_str().unwrap(), &w.base, 100_000);
    assert_eq!(outside(&target), copy);
    let linked: Vec<&str> = target.linked_into_repo.iter().map(|p| p.as_str()).collect();
    assert_eq!(linked, ["src"]);
    let expansion = r.expand_recursive(&target, 1_000);
    assert_eq!(files(&expansion), ["src/a.rs", "src/b.rs", "src/deep/c.rs"]);

    let through = r.resolve(copy.join("live").join("a.rs").to_str().unwrap(), &w.base);
    assert_eq!(in_repo(&through), "src/a.rs");
    assert!(through.via_link);

    // `rm` on the junction itself removes only the junction; with a trailing separator the
    // junction is followed.
    let entry = r.resolve_entry(copy.join("live").to_str().unwrap(), &w.base);
    assert_eq!(outside(&entry), copy.join("live"));
    let followed = r.resolve_entry(&format!(r"{}\", copy.join("live").display()), &w.base);
    assert_eq!(in_repo(&followed), "src");
}

#[test]
fn junction_inside_the_repo_pointing_outside_is_outside() {
    let w = World::new();
    let r = w.resolver();
    dir_link(&w.outside, &w.repo.join("ext"));
    let resolved = r.resolve(r"ext\file.txt", &w.repo);
    assert_eq!(outside(&resolved), w.outside.join("file.txt"));
    assert!(resolved.via_link);
}

#[test]
fn deep_paths_beyond_max_path_resolve() {
    let w = World::new();
    let r = w.resolver();
    let mut deep = PathBuf::from("deep");
    for _ in 0..30 {
        deep.push("a-directory-with-a-rather-long-name");
    }
    let full = w.repo.join(&deep);
    // Created through the verbatim form, which lifts the 260-character limit.
    std::fs::create_dir_all(format!(r"\\?\{}", full.display())).unwrap();
    std::fs::write(format!(r"\\?\{}\f.txt", full.display()), b"x").unwrap();
    let resolved = r.resolve(&format!(r"{}\f.txt", deep.display()), &w.repo);
    assert!(in_repo(&resolved).ends_with("/f.txt"), "{resolved:?}");
    assert!(in_repo(&resolved).starts_with("deep/a-directory"));
}
