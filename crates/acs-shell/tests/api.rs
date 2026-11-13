//! The public surface: `analyze`, `Dialect`, and how the result reads through the helpers of
//! `ShellAnalysis`.

use acs_core::gate::{CommandClass, Confidence, TargetOp};
use acs_shell::{Dialect, analyze};

#[test]
fn an_empty_line_has_no_commands_and_is_complete() {
    for dialect in [Dialect::Bash, Dialect::PowerShell, Dialect::Cmd] {
        let analysis = analyze("", dialect);
        assert!(analysis.commands.is_empty());
        assert!(analysis.complete);
        assert!(!analysis.is_destructive());
        assert!(!analysis.has_unknown_targets());
    }
}

#[test]
fn bash_is_the_default_dialect() {
    assert_eq!(Dialect::default(), Dialect::Bash);
    assert_eq!(
        serde_json::to_string(&Dialect::PowerShell).unwrap_or_default(),
        "\"powershell\""
    );
}

#[test]
fn a_literal_delete_is_destructive_with_known_targets() {
    let analysis = analyze("rm -rf build", Dialect::Bash);
    assert!(analysis.is_destructive());
    assert!(!analysis.has_unknown_targets());
    let command = &analysis.commands[0];
    assert_eq!(command.classes, [CommandClass::Delete]);
    assert_eq!(command.targets.len(), 1);
    let target = &command.targets[0];
    assert_eq!(
        (target.path.as_str(), target.op, target.recursive),
        ("build", TargetOp::Delete, true)
    );
    assert_eq!(target.confidence, Confidence::Exact);
}

#[test]
fn an_expansion_makes_the_target_partial_and_the_line_unknown() {
    let analysis = analyze("rm -rf \"$VAR\"/", Dialect::Bash);
    assert!(analysis.is_destructive());
    assert!(analysis.has_unknown_targets());
    assert_eq!(
        analysis.commands[0].targets[0].confidence,
        Confidence::Partial
    );
    assert_eq!(analysis.commands[0].targets[0].path, "$VAR/");
}

#[test]
fn a_line_that_does_not_parse_is_unknown() {
    let analysis = analyze("rm -rf build &&", Dialect::Bash);
    assert!(!analysis.complete);
    assert!(analysis.has_unknown_targets());
    // What could be recognized is still reported.
    assert_eq!(analysis.commands[0].words, ["rm", "-rf", "build"]);
}

#[test]
fn wrappers_and_assignments_do_not_hide_the_program() {
    let analysis = analyze(
        "FOO=1 sudo -u me nice -n 5 timeout 9 rm -f x",
        Dialect::Bash,
    );
    assert_eq!(analysis.commands.len(), 1);
    assert_eq!(analysis.commands[0].words, ["rm", "-f", "x"]);
    assert_eq!(analysis.commands[0].classes, [CommandClass::Delete]);
}

#[test]
fn a_directory_change_applies_to_later_commands_only_within_its_scope() {
    let analysis = analyze("(cd a && rm x); rm y; cd b; rm z", Dialect::Bash);
    let paths: Vec<&str> = analysis
        .commands
        .iter()
        .flat_map(|c| c.targets.iter().map(|t| t.path.as_str()))
        .collect();
    assert_eq!(paths, ["a/x", "y", "b/z"]);
}

#[test]
fn an_unknown_directory_makes_relative_targets_partial_but_not_absolute_ones() {
    let analysis = analyze("cd \"$D\" && rm rel /abs", Dialect::Bash);
    let rm = &analysis.commands[1];
    assert_eq!(rm.targets[0].confidence, Confidence::Partial);
    assert_eq!(rm.targets[1].confidence, Confidence::Exact);
}

#[test]
fn script_text_is_analyzed_and_the_shell_itself_stays_unknown() {
    let analysis = analyze("bash -c 'rm -rf build'", Dialect::Bash);
    assert_eq!(analysis.commands.len(), 2);
    assert!(
        analysis.commands[0]
            .classes
            .contains(&CommandClass::Unknown)
    );
    assert!(!analysis.commands[0].targets_known);
    assert_eq!(analysis.commands[1].words, ["rm", "-rf", "build"]);
}

#[test]
fn git_facts_are_reported() {
    let analysis = analyze(
        "git -C sub push --force-with-lease origin +main:release",
        Dialect::Bash,
    );
    let git = analysis.commands[0]
        .git
        .as_ref()
        .expect("a git command has git facts");
    assert_eq!(git.subcommand, "push");
    assert!(git.force_push);
    assert!(git.rewrites_history);
    assert!(!git.creates_commit);
    assert_eq!(git.refs, ["release"]);
}

#[test]
fn the_same_program_in_other_dialects() {
    let ps = analyze("Remove-Item -Recurse -Force build", Dialect::PowerShell);
    assert_eq!(ps.commands[0].classes, [CommandClass::Delete]);
    assert!(ps.commands[0].targets[0].recursive);
    let cmd = analyze("rd /s /q build", Dialect::Cmd);
    assert_eq!(cmd.commands[0].classes, [CommandClass::Delete]);
    assert!(cmd.commands[0].targets[0].recursive);
}

/// The directory of the `git` invocation in `line`, or `None` when it is unknown.
fn git_dir(line: &str, dialect: Dialect) -> Option<String> {
    let analysis = analyze(line, dialect);
    let git = analysis.commands.iter().find_map(|c| c.git.as_ref())?;
    (!git.dir_unknown).then(|| git.dir.clone())
}

#[test]
fn a_git_invocation_knows_the_directory_it_runs_in() {
    let bash = |line| git_dir(line, Dialect::Bash);
    assert_eq!(bash("git commit -m x").as_deref(), Some(""));
    assert_eq!(bash("cd wt && git commit -m x").as_deref(), Some("wt"));
    assert_eq!(bash("git -C wt commit -m x").as_deref(), Some("wt"));
    assert_eq!(
        bash("cd a && git -C b -C ../c commit").as_deref(),
        Some("a/c")
    );
    assert_eq!(bash("cd a && git -C .. commit").as_deref(), Some(""));
    assert_eq!(
        bash("cd a; git -C /abs/wt push").as_deref(),
        Some("/abs/wt")
    );
    assert_eq!(
        bash("cd 'C:/work/wt' && git add -A && git commit -m x").as_deref(),
        Some("C:/work/wt")
    );
    assert_eq!(bash("(cd wt && true); git commit").as_deref(), Some(""));
    assert_eq!(bash("cd \"$WT\" && git commit"), None);
    assert_eq!(bash("git -C \"$WT\" commit"), None);
    assert_eq!(
        git_dir("cd wt; git commit -m x", Dialect::PowerShell).as_deref(),
        Some("wt")
    );
}
