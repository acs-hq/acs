//! The hook installer against real repositories and real `git commit` / `git push`.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use acs_git::hooks::{self, Action, Framework, LocationKind, RemoveAction};
use acs_git::{RepoLayout, discover};

struct Repo {
    _tmp: tempfile::TempDir,
    dir: PathBuf,
    config: PathBuf,
    path_var: std::ffi::OsString,
}

fn git_in(dir: &Path, config: &Path, path_var: &std::ffi::OsStr, args: &[&str]) -> Output {
    Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_CONFIG_GLOBAL", config)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("PATH", path_var)
        .output()
        .unwrap()
}

impl Repo {
    fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let base = dunce::canonicalize(tmp.path()).unwrap();
        let dir = base.join("repo");
        fs::create_dir_all(&dir).unwrap();
        let config = base.join("gitconfig");
        fs::write(&config, "[user]\n\temail = t@example.com\n\tname = T\n").unwrap();
        let path_var = path_without_acs();
        let repo = Self {
            _tmp: tmp,
            dir,
            config,
            path_var,
        };
        assert!(
            repo.git(&["init", "-q", "-b", "main", "."])
                .status
                .success()
        );
        repo
    }

    fn git(&self, args: &[&str]) -> Output {
        git_in(&self.dir, &self.config, &self.path_var, args)
    }

    fn layout(&self) -> RepoLayout {
        discover(&self.dir).unwrap()
    }

    fn commit(&self, name: &str) -> Output {
        fs::write(self.dir.join(name), name).unwrap();
        assert!(self.git(&["add", name]).status.success());
        self.git(&["commit", "-q", "-m", name])
    }

    fn hooks_dir(&self) -> PathBuf {
        self.layout().common_dir.join("hooks")
    }
}

/// `PATH` without any directory holding an `acs` binary. These tests rely on `acs` being absent
/// (the hook's "did not run" warning shows that the ACS block executed), but on Windows cargo puts
/// the target directory, which holds the freshly built `acs.exe`, on `PATH` for test processes.
fn path_without_acs() -> std::ffi::OsString {
    let path = std::env::var_os("PATH").unwrap_or_default();
    let kept = std::env::split_paths(&path)
        .filter(|dir| !dir.join("acs").is_file() && !dir.join("acs.exe").is_file());
    std::env::join_paths(kept).unwrap()
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

fn write_executable(path: &Path, text: &str) {
    fs::write(path, text).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
    }
}

#[test]
fn fresh_repo_gets_three_hooks_and_installing_twice_changes_nothing() {
    let repo = Repo::new();
    let first = hooks::install(&repo.layout()).unwrap();
    assert!(first.changed());
    assert_eq!(first.location.kind, LocationKind::Default);
    assert!(first.hooks.iter().all(|h| h.action == Action::Created));
    let names: Vec<_> = first.hooks.iter().map(|h| h.name).collect();
    assert_eq!(names, ["pre-commit", "prepare-commit-msg", "pre-push"]);
    let before: Vec<String> = first
        .hooks
        .iter()
        .map(|h| fs::read_to_string(&h.path).unwrap())
        .collect();
    assert!(before[0].starts_with("#!/bin/sh\n# >>> acs git-hook pre-commit >>>"));
    assert!(before[0].contains("acs git-hook pre-commit \"$@\""));

    let second = hooks::install(&repo.layout()).unwrap();
    assert!(!second.changed(), "{second:?}");
    let after: Vec<String> = second
        .hooks
        .iter()
        .map(|h| fs::read_to_string(&h.path).unwrap())
        .collect();
    assert_eq!(before, after);

    let status = hooks::status(&repo.layout()).unwrap();
    assert!(status.healthy(), "{status:?}");
}

#[test]
fn without_acs_on_path_the_hook_warns_and_git_proceeds() {
    let repo = Repo::new();
    hooks::install(&repo.layout()).unwrap();
    // `acs` is not installed on the test machine, so the hooks must fail open.
    let out = repo.commit("a.txt");
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(stderr(&out).contains("acs: the ACS git backstop (pre-commit) did not run"));
}

#[test]
fn an_existing_shell_hook_is_chained_and_uninstall_restores_it_exactly() {
    let repo = Repo::new();
    let hook = repo.hooks_dir().join("pre-commit");
    fs::create_dir_all(hook.parent().unwrap()).unwrap();
    let original = "#!/bin/sh\n# my hook\ntouch \"$PWD/original-ran\"\n";
    write_executable(&hook, original);

    let report = hooks::install(&repo.layout()).unwrap();
    let change = report
        .hooks
        .iter()
        .find(|h| h.name == "pre-commit")
        .unwrap();
    assert_eq!(change.action, Action::Injected);
    let text = fs::read_to_string(&hook).unwrap();
    assert!(text.starts_with("#!/bin/sh\n# >>> acs git-hook"), "{text}");
    assert!(text.contains("touch \"$PWD/original-ran\""));

    let out = repo.commit("a.txt");
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(
        repo.dir.join("original-ran").exists(),
        "the original hook must still run"
    );

    assert!(!hooks::install(&repo.layout()).unwrap().changed());
    let status = hooks::status(&repo.layout()).unwrap();
    let entry = status
        .hooks
        .iter()
        .find(|h| h.name == "pre-commit")
        .unwrap();
    assert!(entry.installed && entry.chained && entry.up_to_date && entry.active);

    let removed = hooks::uninstall(&repo.layout()).unwrap();
    let pre_commit = removed.iter().find(|r| r.name == "pre-commit").unwrap();
    assert_eq!(pre_commit.action, RemoveAction::Removed);
    assert_eq!(fs::read_to_string(&hook).unwrap(), original);
    // The hooks ACS created are gone.
    assert!(!repo.hooks_dir().join("pre-push").exists());
    assert_eq!(
        removed
            .iter()
            .filter(|r| r.action == RemoveAction::Deleted)
            .count(),
        2
    );
    // And a second uninstall has nothing to do.
    assert!(
        hooks::uninstall(&repo.layout())
            .unwrap()
            .iter()
            .all(|r| r.action == RemoveAction::NotInstalled)
    );
}

#[test]
fn an_outdated_block_is_replaced() {
    let repo = Repo::new();
    hooks::install(&repo.layout()).unwrap();
    let hook = repo.hooks_dir().join("pre-commit");
    let text = fs::read_to_string(&hook).unwrap();
    let old = text.replace(" v1 ", " v0 ").replace("exit $?", "exit 0");
    fs::write(&hook, &old).unwrap();
    assert!(!hooks::status(&repo.layout()).unwrap().healthy());
    let report = hooks::install(&repo.layout()).unwrap();
    let change = report
        .hooks
        .iter()
        .find(|h| h.name == "pre-commit")
        .unwrap();
    assert_eq!(change.action, Action::Updated);
    assert_eq!(fs::read_to_string(&hook).unwrap(), text);
}

#[test]
fn husky_v9_user_scripts_get_the_block_and_the_stubs_are_left_alone() {
    let repo = Repo::new();
    let husky = repo.dir.join(".husky");
    let stubs = husky.join("_");
    fs::create_dir_all(&stubs).unwrap();
    // The runner as husky 9.1 writes it.
    fs::write(
        stubs.join("h"),
        "#!/usr/bin/env sh\n\
         n=$(basename \"$0\")\n\
         s=$(dirname \"$(dirname \"$0\")\")/$n\n\
         [ ! -f \"$s\" ] && exit 0\n\
         [ \"${HUSKY-}\" = \"0\" ] && exit 0\n\
         sh -e \"$s\" \"$@\"\n\
         c=$?\n\
         [ $c != 0 ] && echo \"husky - $n script failed (code $c)\"\n\
         exit $c\n",
    )
    .unwrap();
    fs::write(stubs.join(".gitignore"), "*").unwrap();
    for name in ["pre-commit", "prepare-commit-msg", "pre-push"] {
        write_executable(
            &stubs.join(name),
            "#!/usr/bin/env sh\n. \"$(dirname \"$0\")/h\"",
        );
    }
    fs::write(husky.join("pre-commit"), "touch husky-ran\n").unwrap();
    assert!(
        repo.git(&["config", "core.hooksPath", ".husky/_"])
            .status
            .success()
    );

    let report = hooks::install(&repo.layout()).unwrap();
    assert_eq!(report.location.kind, LocationKind::HuskyV9);
    assert_eq!(
        report.location.scripts_dir,
        dunce::canonicalize(&husky).unwrap()
    );
    let pre_commit = report
        .hooks
        .iter()
        .find(|h| h.name == "pre-commit")
        .unwrap();
    assert_eq!(pre_commit.action, Action::Injected);
    let pre_push = report.hooks.iter().find(|h| h.name == "pre-push").unwrap();
    assert_eq!(pre_push.action, Action::Created);

    let out = repo.commit("a.txt");
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(
        repo.dir.join("husky-ran").exists(),
        "the user's husky script must still run"
    );
    assert!(stderr(&out).contains("did not run"), "ACS ran first");
    let stub = fs::read_to_string(stubs.join("pre-commit")).unwrap();
    assert_eq!(stub, "#!/usr/bin/env sh\n. \"$(dirname \"$0\")/h\"");

    let status = hooks::status(&repo.layout()).unwrap();
    assert_eq!(status.framework, Some(Framework::Husky));
    assert!(status.healthy(), "{status:?}");
    assert!(!hooks::install(&repo.layout()).unwrap().changed());

    hooks::uninstall(&repo.layout()).unwrap();
    assert_eq!(
        fs::read_to_string(husky.join("pre-commit")).unwrap(),
        "touch husky-ran\n"
    );
    assert!(!husky.join("pre-push").exists());
}

#[test]
fn husky_v8_hooks_get_the_block_after_the_husky_sh_line() {
    let repo = Repo::new();
    let husky = repo.dir.join(".husky");
    fs::create_dir_all(husky.join("_")).unwrap();
    fs::write(husky.join("_").join("husky.sh"), "# husky helper\n").unwrap();
    let original = "#!/usr/bin/env sh\n. \"$(dirname -- \"$0\")/_/husky.sh\"\n\ntouch husky8-ran\n";
    write_executable(&husky.join("pre-commit"), original);
    assert!(
        repo.git(&["config", "core.hooksPath", ".husky"])
            .status
            .success()
    );

    let report = hooks::install(&repo.layout()).unwrap();
    assert_eq!(report.location.kind, LocationKind::HuskyLegacy);
    let text = fs::read_to_string(husky.join("pre-commit")).unwrap();
    let husky_line = text.find("husky.sh").unwrap();
    let block = text.find("# >>> acs git-hook").unwrap();
    assert!(husky_line < block, "{text}");

    let out = repo.commit("a.txt");
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(repo.dir.join("husky8-ran").exists());
    hooks::uninstall(&repo.layout()).unwrap();
    assert_eq!(
        fs::read_to_string(husky.join("pre-commit")).unwrap(),
        original
    );
}

#[test]
fn the_pre_commit_framework_script_is_chained() {
    let repo = Repo::new();
    let hook = repo.hooks_dir().join("pre-commit");
    fs::create_dir_all(hook.parent().unwrap()).unwrap();
    let original = "#!/usr/bin/env bash\n\
                    # File generated by pre-commit: https://pre-commit.com\n\
                    # ID: 138fd403232d2ddd5efb44317e38bf03\n\
                    touch framework-ran\n\
                    exit 0\n";
    write_executable(&hook, original);
    hooks::install(&repo.layout()).unwrap();
    let status = hooks::status(&repo.layout()).unwrap();
    assert_eq!(status.framework, Some(Framework::PreCommit));
    let out = repo.commit("a.txt");
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(repo.dir.join("framework-ran").exists());
    hooks::uninstall(&repo.layout()).unwrap();
    assert_eq!(fs::read_to_string(&hook).unwrap(), original);
}

#[test]
fn core_hooks_path_absolute_and_relative_are_respected() {
    let repo = Repo::new();
    let abs = repo.dir.parent().unwrap().join("shared-hooks");
    assert!(
        repo.git(&["config", "core.hooksPath", abs.to_str().unwrap()])
            .status
            .success()
    );
    let report = hooks::install(&repo.layout()).unwrap();
    assert_eq!(report.location.kind, LocationKind::HooksPath);
    assert!(abs.join("pre-commit").is_file());
    assert!(!repo.hooks_dir().join("pre-commit").exists());

    assert!(
        repo.git(&["config", "core.hooksPath", ".githooks"])
            .status
            .success()
    );
    let report = hooks::install(&repo.layout()).unwrap();
    assert!(repo.dir.join(".githooks/pre-push").is_file());
    assert!(!report.location.shared_by_worktrees);
    let out = repo.commit("a.txt");
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(
        stderr(&out).contains("did not run"),
        "the relative hooksPath hook ran"
    );
}

#[test]
fn hooks_disabled_with_dev_null_are_reported() {
    let repo = Repo::new();
    assert!(
        repo.git(&["config", "core.hooksPath", "/dev/null"])
            .status
            .success()
    );
    assert!(matches!(
        hooks::install(&repo.layout()),
        Err(hooks::HooksError::Disabled(_))
    ));
}

#[test]
fn linked_worktrees_share_the_hooks_of_the_common_directory() {
    let repo = Repo::new();
    assert!(repo.commit("a.txt").status.success());
    let linked = repo.dir.parent().unwrap().join("linked");
    assert!(
        repo.git(&[
            "worktree",
            "add",
            "-q",
            "-b",
            "side",
            linked.to_str().unwrap()
        ])
        .status
        .success()
    );
    let report = hooks::install(&discover(&linked).unwrap()).unwrap();
    assert!(report.location.shared_by_worktrees);
    assert_eq!(report.location.scripts_dir, repo.hooks_dir());
    // Installing from the main work tree afterwards finds everything in place.
    assert!(!hooks::install(&repo.layout()).unwrap().changed());
    // Both worktrees run the hooks.
    let out = git_in(
        &linked,
        &repo.config,
        &repo.path_var,
        &["commit", "-q", "--allow-empty", "-m", "x"],
    );
    assert!(out.status.success());
    assert!(stderr(&out).contains("did not run"));
}

#[cfg(unix)]
mod unix_only {
    use super::*;

    fn fake_acs(dir: &Path) -> std::ffi::OsString {
        let bin = dir.join("fake-bin");
        fs::create_dir_all(&bin).unwrap();
        write_executable(
            &bin.join("acs"),
            "#!/bin/sh\n\
             echo \"$@\" >> \"$ACS_FAKE_LOG\"\n\
             if [ \"$2\" = pre-push ]; then cat >> \"$ACS_FAKE_LOG\"; fi\n\
             exit \"${ACS_FAKE_EXIT:-0}\"\n",
        );
        let mut paths = vec![bin];
        paths.extend(std::env::split_paths(
            &std::env::var_os("PATH").unwrap_or_default(),
        ));
        std::env::join_paths(paths).unwrap()
    }

    fn run(repo: &Repo, log: &Path, exit: &str, args: &[&str]) -> Output {
        Command::new("git")
            .arg("-C")
            .arg(&repo.dir)
            .args(args)
            .env("GIT_CONFIG_GLOBAL", &repo.config)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("PATH", &repo.path_var)
            .env("ACS_FAKE_LOG", log)
            .env("ACS_FAKE_EXIT", exit)
            .output()
            .unwrap()
    }

    #[test]
    fn the_hooks_call_acs_with_git_s_arguments_and_a_failure_blocks_git() {
        let mut repo = Repo::new();
        repo.path_var = fake_acs(repo.dir.parent().unwrap());
        let log = repo.dir.parent().unwrap().join("log");
        hooks::install(&repo.layout()).unwrap();
        fs::write(repo.dir.join("a.txt"), "a").unwrap();
        run(&repo, &log, "0", &["add", "a.txt"]);
        let ok = run(&repo, &log, "0", &["commit", "-q", "-m", "first"]);
        assert!(ok.status.success(), "{}", stderr(&ok));
        let logged = fs::read_to_string(&log).unwrap();
        assert!(logged.contains("git-hook pre-commit"), "{logged}");
        assert!(logged.contains("git-hook prepare-commit-msg"), "{logged}");
        assert!(logged.contains("COMMIT_EDITMSG message"), "{logged}");

        fs::write(repo.dir.join("b.txt"), "b").unwrap();
        run(&repo, &log, "0", &["add", "b.txt"]);
        let blocked = run(&repo, &log, "1", &["commit", "-q", "-m", "second"]);
        assert!(
            !blocked.status.success(),
            "a non-zero exit of acs must stop the commit"
        );
    }

    #[test]
    fn pre_push_gives_stdin_to_acs_and_to_the_original_hook() {
        let mut repo = Repo::new();
        repo.path_var = fake_acs(repo.dir.parent().unwrap());
        let log = repo.dir.parent().unwrap().join("log");
        let seen = repo.dir.parent().unwrap().join("original-stdin");
        let hook = repo.hooks_dir().join("pre-push");
        fs::create_dir_all(hook.parent().unwrap()).unwrap();
        write_executable(
            &hook,
            &format!("#!/bin/sh\ncat > '{}'\nexit 0\n", seen.display()),
        );
        hooks::install(&repo.layout()).unwrap();
        assert!(repo.commit("a.txt").status.success());
        let remote = repo.dir.parent().unwrap().join("remote.git");
        assert!(
            Command::new("git")
                .args(["init", "-q", "--bare"])
                .arg(&remote)
                .status()
                .unwrap()
                .success()
        );
        assert!(
            repo.git(&["remote", "add", "origin", remote.to_str().unwrap()])
                .status
                .success()
        );
        let out = run(&repo, &log, "0", &["push", "-q", "origin", "main"]);
        assert!(out.status.success(), "{}", stderr(&out));
        let logged = fs::read_to_string(&log).unwrap();
        assert!(logged.contains("git-hook pre-push origin"), "{logged}");
        assert!(
            logged.contains("refs/heads/main"),
            "stdin reached acs: {logged}"
        );
        let original_saw = fs::read_to_string(&seen).unwrap();
        assert!(
            original_saw.contains("refs/heads/main"),
            "stdin reached the original hook"
        );
    }

    #[test]
    fn a_non_shell_hook_is_wrapped_and_restored() {
        if !Path::new("/usr/bin/awk").exists() {
            return;
        }
        let repo = Repo::new();
        let hook = repo.hooks_dir().join("pre-commit");
        fs::create_dir_all(hook.parent().unwrap()).unwrap();
        let original = "#!/usr/bin/awk -f\nBEGIN { print \"x\" > \"awk-ran\"; exit 0 }\n";
        write_executable(&hook, original);
        let report = hooks::install(&repo.layout()).unwrap();
        let change = report
            .hooks
            .iter()
            .find(|h| h.name == "pre-commit")
            .unwrap();
        assert_eq!(change.action, Action::Wrapped);
        assert_eq!(
            fs::read_to_string(repo.hooks_dir().join("pre-commit.acs-orig")).unwrap(),
            original
        );
        assert!(!hooks::install(&repo.layout()).unwrap().changed());
        let status = hooks::status(&repo.layout()).unwrap();
        assert!(status.hooks[0].chained);

        let out = repo.commit("a.txt");
        assert!(out.status.success(), "{}", stderr(&out));
        assert!(
            repo.dir.join("awk-ran").exists(),
            "the original (non-shell) hook ran"
        );

        let removed = hooks::uninstall(&repo.layout()).unwrap();
        assert_eq!(removed[0].action, RemoveAction::Restored);
        assert_eq!(fs::read_to_string(&hook).unwrap(), original);
        assert!(!repo.hooks_dir().join("pre-commit.acs-orig").exists());
    }

    #[test]
    fn a_hook_that_is_not_executable_is_reported_inactive() {
        let repo = Repo::new();
        let hook = repo.hooks_dir().join("pre-commit");
        fs::create_dir_all(hook.parent().unwrap()).unwrap();
        fs::write(&hook, "#!/bin/sh\nexit 0\n").unwrap();
        hooks::install(&repo.layout()).unwrap();
        let status = hooks::status(&repo.layout()).unwrap();
        let entry = status
            .hooks
            .iter()
            .find(|h| h.name == "pre-commit")
            .unwrap();
        assert!(entry.installed && !entry.active);
        assert!(!status.healthy());
    }
}
