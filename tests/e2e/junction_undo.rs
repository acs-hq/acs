//! The 48,000-file incident, recovered: the delete went through the junction anyway (here the
//! gate is off), and `acs undo --apply` brings every file of the live tree back byte for byte,
//! with its mode, empty files included.
//!
//! What the real pipeline does here: the pre-tool step expands `rm -rf scratch` to the files
//! under the directory, junction included, and takes a pre-image of each under its real path
//! (`src/lib.rs`, not `scratch/copy/live/lib.rs`). Nothing in this test fakes a pre-image.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use crate::common::{Agent, World, git};

fn dir_link(target: &Path, link: &Path) {
    #[cfg(unix)]
    std::os::unix::fs::symlink(target, link).unwrap();
    #[cfg(windows)]
    {
        let out = std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(link)
            .arg(target)
            .output()
            .unwrap();
        assert!(out.status.success(), "mklink /J failed: {out:?}");
    }
}

fn set_readonly(path: &Path, readonly: bool) {
    let mut perms = fs::metadata(path).unwrap().permissions();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        perms.set_mode(if readonly { 0o444 } else { 0o644 });
    }
    #[cfg(not(unix))]
    {
        #[allow(clippy::permissions_set_readonly_false)]
        perms.set_readonly(readonly);
    }
    fs::set_permissions(path, perms).unwrap();
}

/// Every file under `dir` (relative path -> bytes and whether it is writable).
fn tree(dir: &Path) -> BTreeMap<String, (Vec<u8>, bool, bool)> {
    fn walk(root: &Path, dir: &Path, out: &mut BTreeMap<String, (Vec<u8>, bool, bool)>) {
        for entry in fs::read_dir(dir).unwrap() {
            let path: PathBuf = entry.unwrap().path();
            if path.is_dir() {
                walk(root, &path, out);
            } else {
                let meta = fs::metadata(&path).unwrap();
                #[cfg(unix)]
                let exec = {
                    use std::os::unix::fs::PermissionsExt;
                    meta.permissions().mode() & 0o111 != 0
                };
                #[cfg(not(unix))]
                let exec = false;
                let rel = path
                    .strip_prefix(root)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/");
                out.insert(
                    rel,
                    (
                        fs::read(&path).unwrap(),
                        meta.permissions().readonly(),
                        exec,
                    ),
                );
            }
        }
    }
    let mut out = BTreeMap::new();
    walk(dir, dir, &mut out);
    out
}

#[test]
fn files_deleted_through_a_junction_come_back_byte_exact_with_acs_undo() {
    let world = World::new();
    // The live tree: an empty file, a binary file, a read-only file, a script.
    fs::write(world.repo.join("src/empty.rs"), "").unwrap();
    fs::write(
        world.repo.join("src/blob.bin"),
        [0u8, 159, 146, 150, 255, 13, 10, 0],
    )
    .unwrap();
    fs::create_dir_all(world.repo.join("src/deep/er")).unwrap();
    fs::write(world.repo.join("src/deep/er/leaf.rs"), "// leaf\r\n").unwrap();
    set_readonly(&world.repo.join("src/util.rs"), true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::write(world.repo.join("src/run.sh"), "#!/bin/sh\n").unwrap();
        fs::set_permissions(
            world.repo.join("src/run.sh"),
            fs::Permissions::from_mode(0o755),
        )
        .unwrap();
    }
    git(&world.repo, &world.git_config, &["add", "-A"]);
    git(
        &world.repo,
        &world.git_config,
        &["commit", "-q", "-m", "live tree"],
    );

    let copy = world.repo.join("scratch").join("copy");
    fs::create_dir_all(&copy).unwrap();
    fs::write(copy.join("notes.txt"), "scratch\n").unwrap();
    dir_link(&world.repo.join("src"), &copy.join("live"));
    let live_before = tree(&world.repo.join("src"));
    assert!(live_before.len() >= 5, "{live_before:?}");

    // The gate is off, so the delete is allowed; the hook still takes pre-images.
    assert!(world.run(&["off"], b"", &world.repo).status.success());
    let agent = Agent::new(&world, "claude-code", "s1");
    agent.start();
    let command = "rm -rf scratch";
    let decision = agent.pre_shell(command);
    assert_eq!(decision.outcome, acs_core::Outcome::Allow, "{decision:?}");

    // The delete goes through the junction into the live tree.
    for rel in live_before.keys() {
        let file = copy.join("live").join(rel);
        set_readonly(&file, false);
        fs::remove_file(&file).unwrap();
    }
    fs::remove_dir_all(world.repo.join("src/deep")).unwrap();
    fs::remove_dir(world.repo.join("src")).unwrap();
    fs::remove_dir_all(world.repo.join("scratch")).unwrap();
    agent.post_shell(command);
    agent.end();
    assert!(!world.repo.join("src").exists());

    let sid = agent.info().session_id.to_string();
    let dry = world.run(&["undo", &sid], b"", &world.repo);
    assert!(dry.status.success(), "{dry:?}");
    let plan = String::from_utf8_lossy(&dry.stdout).into_owned();
    assert!(plan.contains("recreate"), "{plan}");
    assert!(
        !world.repo.join("src").exists(),
        "a dry run changes nothing"
    );

    let out = world.run(&["undo", &sid, "--apply"], b"", &world.repo);
    assert!(out.status.success(), "{out:?}");
    let shown = String::from_utf8_lossy(&out.stdout).into_owned();
    assert!(shown.lines().all(|l| l.chars().count() <= 80), "{shown}");

    assert_eq!(tree(&world.repo.join("src")), live_before, "{shown}");
    assert_eq!(
        fs::read_to_string(copy.join("notes.txt")).unwrap(),
        "scratch\n"
    );
    let status = std::process::Command::new("git")
        .arg("-C")
        .arg(&world.repo)
        .args(["status", "--porcelain", "--", "src"])
        .output()
        .unwrap();
    assert_eq!(
        String::from_utf8_lossy(&status.stdout).trim(),
        "",
        "src is as committed"
    );
}
