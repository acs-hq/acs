//! Behavior of the policy loader through its public API: defaults, validation errors, the
//! `PolicyView` implementation, the cache and its fallbacks.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::fs;
use std::path::Path;
use std::time::Instant;

use acs_core::gate::PolicyView;
use acs_core::{Config, DEFAULT_SEALED, Guards, Owner, RepoPath, Territory, ZoneKind};
use acs_policy::{
    CACHE_FILE, CompiledPolicy, DEFAULT_BAN, LoadOptions, Origin, PolicyError, PolicyFile,
    ProblemKind, builtin_guards, load, load_cached, policy_hash,
};
use tempfile::TempDir;

const CS: LoadOptions = LoadOptions {
    case_insensitive: false,
};
const CI: LoadOptions = LoadOptions {
    case_insensitive: true,
};

fn rp(path: &str) -> RepoPath {
    path.parse().unwrap()
}

/// A policy directory with the given `(file name, content)` pairs.
fn policy_dir(files: &[(&str, &str)]) -> TempDir {
    let dir = tempfile::tempdir().unwrap();
    for (name, content) in files {
        fs::write(dir.path().join(name), content).unwrap();
    }
    dir
}

fn load_ok(files: &[(&str, &str)], opts: LoadOptions) -> CompiledPolicy {
    let dir = policy_dir(files);
    load(dir.path(), opts)
        .unwrap_or_else(|e| panic!("{e}"))
        .policy
}

// ---------------------------------------------------------------------------------------------
// Defaults
// ---------------------------------------------------------------------------------------------

#[test]
fn absent_policy_directory_yields_the_built_in_defaults() {
    let dir = tempfile::tempdir().unwrap();
    let loaded = load(&dir.path().join("does-not-exist"), CS).unwrap();
    assert!(loaded.warnings.is_empty());
    let policy = loaded.policy;
    assert_eq!(policy.config(), &Config::default());
    assert_eq!(policy.files().guards, builtin_guards());
    assert_eq!(policy.files().territory, Territory::default());
    // The hash of "all three files absent" is what the built-in policy reports.
    assert_eq!(policy.hash(), CompiledPolicy::builtin(false).hash());
    assert_eq!(policy.hash(), &policy_hash(dir.path()).unwrap());
}

#[test]
fn the_built_in_policy_compiles_for_both_case_settings() {
    for ci in [false, true] {
        let policy = CompiledPolicy::builtin(ci);
        assert_eq!(policy.case_insensitive(), ci);
        assert_eq!(policy.files(), &acs_policy::PolicyFiles::builtin());
        assert!(policy.sealed_match(&rp(".git/config")).is_some());
    }
}

#[test]
fn default_guards() {
    let policy = load_ok(&[], CS);
    // Sealed: every default glob matches a representative path.
    let sealed_paths = [
        (".git/config", ".git/**"),
        (".git", ".git/**"),
        (".acs/policy/guards.toml", ".acs/**"),
        (".github/workflows/ci.yml", ".github/workflows/**"),
        (".env", "**/.env*"),
        ("services/api/.env.production", "**/.env*"),
        (".claude/settings.json", "**/.claude/settings*.json"),
        (".claude/settings.local.json", "**/.claude/settings*.json"),
        (
            "apps/web/.claude/settings.json",
            "**/.claude/settings*.json",
        ),
        (".claude/hooks/guard.sh", "**/.claude/hooks/**"),
        (".agents/hooks.json", "**/.agents/**/hooks*.json"),
        (".agents/foo/hooks.json", "**/.agents/**/hooks*.json"),
        (".agent/hooks.json", "**/.agent/**/hooks*.json"),
        (".agents/plugins/acs/plugin.json", "**/.agents/plugins/**"),
        (".agent/plugins/x/hooks.json", "**/.agent/**/hooks*.json"),
        (".agent/plugins/x/plugin.json", "**/.agent/plugins/**"),
        (".agents/plugins.json", "**/.agents/plugins.json"),
        (".agent/plugins.json", "**/.agent/plugins.json"),
    ];
    for (path, glob) in sealed_paths {
        assert_eq!(policy.sealed_match(&rp(path)), Some(glob), "{path}");
    }
    for path in [
        "src/main.rs",
        "docs/env.md",
        ".github/CODEOWNERS",
        ".claude/other.json",
        ".claude/worktrees/wp/src/main.rs",
        ".mcp.json",
        ".agents/rules/acs.md",
    ] {
        assert_eq!(policy.sealed_match(&rp(path)), None, "{path}");
    }
    assert_eq!(DEFAULT_SEALED.len(), 12, "update the table above");

    // Protected branches.
    for branch in ["main", "release/1.0", "release/v2"] {
        assert!(policy.is_protected_branch(branch), "{branch}");
    }
    for branch in [
        "feature/x",
        "release/1.0/hotfix",
        "release",
        "mainline",
        "Main",
    ] {
        assert!(!policy.is_protected_branch(branch), "{branch}");
    }

    // Destructive limits and bans.
    let destructive = policy.destructive();
    assert_eq!(destructive.max_files_deleted_outside_claims, 0);
    assert_eq!(destructive.max_files_deleted_per_command, 50);
    assert_eq!(destructive.ban, DEFAULT_BAN);
    assert_eq!(destructive.ban.len(), 3);

    // No zones: everything is shared.
    assert!(policy.zone(&rp("src/main.rs")).is_none());
}

#[test]
fn default_bans_are_full_match_patterns() {
    let policy = load_ok(&[], CS);
    let cases: &[(&str, Option<&str>)] = &[
        ("git push --force", Some("git push --force*")),
        (
            "git push --force-with-lease origin main",
            Some("git push --force*"),
        ),
        ("git push --force origin main", Some("git push --force*")),
        ("git reset --hard", Some("git reset --hard*")),
        ("git reset --hard HEAD~1", Some("git reset --hard*")),
        ("git clean -fdx", Some("git clean -f*")),
        ("git clean -f", Some("git clean -f*")),
        // Not banned.
        ("git push origin main", None),
        ("git reset --soft HEAD~1", None),
        ("git clean -n", None),
        ("echo git push --force", None),
        ("git status", None),
        ("", None),
    ];
    for &(command, expected) in cases {
        assert_eq!(policy.banned_match(command), expected, "{command:?}");
    }
}

// ---------------------------------------------------------------------------------------------
// guards.toml: how a present file combines with the defaults
// ---------------------------------------------------------------------------------------------

#[test]
fn guards_file_adds_to_sealed_and_keeps_defaults_for_absent_fields() {
    let policy = load_ok(
        &[("guards.toml", "sealed = [\"secrets/**\", \".git/**\"]\n")],
        CS,
    );
    assert_eq!(
        policy.sealed_match(&rp("secrets/key.pem")),
        Some("secrets/**")
    );
    // Defaults still apply and come first.
    assert_eq!(policy.sealed_match(&rp(".git/HEAD")), Some(".git/**"));
    assert_eq!(policy.sealed_match(&rp(".acs/policy/x")), Some(".acs/**"));
    // A duplicate of a default is not repeated.
    let sealed = &policy.files().guards.sealed;
    assert_eq!(sealed.iter().filter(|g| *g == ".git/**").count(), 1);
    assert_eq!(sealed.len(), DEFAULT_SEALED.len() + 1);
    // Absent fields keep their defaults.
    assert!(policy.is_protected_branch("main"));
    assert_eq!(policy.destructive().ban, DEFAULT_BAN);
}

#[test]
fn sealed_defaults_cannot_be_removed() {
    // An empty `sealed` list, and a policy compiled directly with no sealed globs at all.
    let policy = load_ok(&[("guards.toml", "sealed = []\n")], CS);
    assert_eq!(
        policy.sealed_match(&rp(".acs/policy/guards.toml")),
        Some(".acs/**")
    );
    let direct = CompiledPolicy::compile(
        Config::default(),
        Guards::default(),
        Territory::default(),
        false,
    )
    .unwrap();
    assert_eq!(direct.sealed_match(&rp(".git/config")), Some(".git/**"));
    // Compiling directly does not add the other built-in defaults.
    assert!(!direct.is_protected_branch("main"));
    assert_eq!(direct.banned_match("git push --force"), None);
}

#[test]
fn guards_fields_present_in_the_file_replace_their_defaults() {
    let policy = load_ok(
        &[(
            "guards.toml",
            r#"
protected_branches = ["trunk"]

[destructive]
max_files_deleted_outside_claims = 3
max_files_deleted_per_command = 10
ban = ["rm -rf /*"]
"#,
        )],
        CS,
    );
    assert!(policy.is_protected_branch("trunk"));
    assert!(!policy.is_protected_branch("main"));
    let destructive = policy.destructive();
    assert_eq!(destructive.max_files_deleted_outside_claims, 3);
    assert_eq!(destructive.max_files_deleted_per_command, 10);
    assert_eq!(policy.banned_match("rm -rf /usr"), Some("rm -rf /*"));
    assert_eq!(policy.banned_match("git push --force"), None);
}

#[test]
fn empty_lists_in_the_file_switch_the_defaults_off() {
    let policy = load_ok(
        &[(
            "guards.toml",
            "protected_branches = []\n[destructive]\nban = []\n",
        )],
        CS,
    );
    assert!(!policy.is_protected_branch("main"));
    assert_eq!(policy.banned_match("git push --force"), None);
    // The numeric limits still default.
    assert_eq!(policy.destructive().max_files_deleted_per_command, 50);
}

#[test]
fn a_destructive_table_without_ban_keeps_the_default_bans() {
    let policy = load_ok(
        &[(
            "guards.toml",
            "[destructive]\nmax_files_deleted_per_command = 5\n",
        )],
        CS,
    );
    assert_eq!(policy.destructive().max_files_deleted_per_command, 5);
    assert_eq!(policy.destructive().ban, DEFAULT_BAN);
}

#[test]
fn protected_branch_globs_use_the_path_dialect() {
    let policy = load_ok(
        &[(
            "guards.toml",
            "protected_branches = [\"main\", \"release/*\", \"hotfix/**\"]\n",
        )],
        CS,
    );
    for (branch, expected) in [
        ("main", true),
        ("release/1", true),
        ("release/1/2", false),
        ("hotfix/1/2", true),
        ("hotfix", true),
        ("feature", false),
    ] {
        assert_eq!(policy.is_protected_branch(branch), expected, "{branch}");
    }
}

// ---------------------------------------------------------------------------------------------
// config.toml
// ---------------------------------------------------------------------------------------------

#[test]
fn config_values_are_loaded() {
    let policy = load_ok(
        &[(
            "config.toml",
            "spec = \"acs/0.1\"\ndefault_target_branch = \"trunk\"\nclaim_ttl_secs = 60\n\
             sync_interval_secs = 5\n[ledger]\nstore_prompts = true\n",
        )],
        CS,
    );
    let config = policy.config();
    assert_eq!(config.default_target_branch, "trunk");
    assert_eq!(config.claim_ttl_secs.get(), 60);
    assert_eq!(config.sync_interval_secs.get(), 5);
    assert!(config.ledger.store_prompts);
    // A minimal file is all defaults.
    let minimal = load_ok(&[("config.toml", "spec = \"acs/0.1\"")], CS);
    assert_eq!(minimal.config(), &Config::default());
}

// ---------------------------------------------------------------------------------------------
// territory.toml: zones, specificity, teams
// ---------------------------------------------------------------------------------------------

const TERRITORY: &str = r#"
[[zone]]
paths = ["payments/**"]
kind = "owned"
owners = ["Sara@Example.com", "team:platform"]

[[zone]]
paths = ["payments/api/**"]
kind = "shared"

[[zone]]
paths = ["**/*.md"]
kind = "open"

[[zone]]
paths = ["payments/docs/**"]
kind = "open"
owners = ["docs@example.com"]

[[zone]]
paths = ["vendor/**", "third_party/**"]
kind = "sealed"

[[zone]]
paths = ["src/*.rs"]
kind = "owned"
owners = ["team:platform"]

[[zone]]
paths = ["src/**/*.rs"]
kind = "shared"

[[zone]]
paths = ["src/**"]
kind = "open"

[teams]
platform = ["omar@example.com", "Lina@Example.com", "omar@example.com"]
empty = []
"#;

/// `(path, expected kind, expected matched pattern)`; `None` means no zone.
#[allow(clippy::type_complexity)]
const ZONE_CASES: &[(&str, Option<(ZoneKind, &str)>)] = &[
    // Plain ownership, and the more specific nested zone.
    ("payments/ledger.rs", Some((ZoneKind::Owned, "payments/**"))),
    (
        "payments/api/charge.rs",
        Some((ZoneKind::Shared, "payments/api/**")),
    ),
    (
        "payments/api/v2/deep/charge.rs",
        Some((ZoneKind::Shared, "payments/api/**")),
    ),
    ("payments", Some((ZoneKind::Owned, "payments/**"))),
    // More leading literal segments beat `**/*.md`; the .md glob still wins elsewhere.
    (
        "payments/docs/readme.md",
        Some((ZoneKind::Open, "payments/docs/**")),
    ),
    ("payments/readme.md", Some((ZoneKind::Owned, "payments/**"))),
    ("docs/guide.md", Some((ZoneKind::Open, "**/*.md"))),
    ("README.md", Some((ZoneKind::Open, "**/*.md"))),
    // Several paths in one zone.
    ("vendor/lib/a.c", Some((ZoneKind::Sealed, "vendor/**"))),
    ("third_party/x", Some((ZoneKind::Sealed, "third_party/**"))),
    // Same leading literals: no `**` beats `**`; then more literal characters beat fewer.
    ("src/main.rs", Some((ZoneKind::Owned, "src/*.rs"))),
    ("src/a/b.rs", Some((ZoneKind::Shared, "src/**/*.rs"))),
    ("src/a/b.toml", Some((ZoneKind::Open, "src/**"))),
    ("src/notes.md", Some((ZoneKind::Open, "src/**"))),
    // Nothing matches.
    ("tools/build.sh", None),
    ("paymentsx/a.rs", None),
];

#[test]
fn zone_lookup_picks_the_most_specific_match() {
    let policy = load_ok(&[("territory.toml", TERRITORY)], CS);
    for &(path, expected) in ZONE_CASES {
        let got = policy.zone(&rp(path)).map(|z| (z.kind, z.pattern));
        assert_eq!(got, expected, "{path}");
    }
}

#[test]
fn equally_specific_zones_resolve_to_the_later_one() {
    let policy = load_ok(
        &[(
            "territory.toml",
            r#"
[[zone]]
paths = ["lib/**"]
kind = "shared"

[[zone]]
paths = ["lib/**"]
kind = "open"

[[zone]]
paths = ["a/*", "a/*"]
kind = "sealed"
"#,
        )],
        CS,
    );
    assert_eq!(policy.zone(&rp("lib/x")).unwrap().kind, ZoneKind::Open);
    assert_eq!(policy.zone(&rp("a/x")).unwrap().kind, ZoneKind::Sealed);
}

#[test]
fn braces_are_ranked_per_alternative() {
    // `{docs,src}/**` has two alternatives of equal rank; the single-literal `docs/api/**`
    // is more specific for its own paths.
    let policy = load_ok(
        &[(
            "territory.toml",
            r#"
[[zone]]
paths = ["{docs,src}/**"]
kind = "shared"

[[zone]]
paths = ["docs/api/**"]
kind = "open"
"#,
        )],
        CS,
    );
    assert_eq!(policy.zone(&rp("src/a")).unwrap().kind, ZoneKind::Shared);
    assert_eq!(policy.zone(&rp("src/a")).unwrap().pattern, "{docs,src}/**");
    assert_eq!(policy.zone(&rp("docs/a")).unwrap().kind, ZoneKind::Shared);
    assert_eq!(policy.zone(&rp("docs/api/a")).unwrap().kind, ZoneKind::Open);
}

#[test]
fn owners_are_reported_as_written_and_expanded_to_lowercase_pilots() {
    let policy = load_ok(&[("territory.toml", TERRITORY)], CS);

    let payments = policy.zone(&rp("payments/ledger.rs")).unwrap();
    assert_eq!(
        payments.owners,
        [
            Owner::Pilot("Sara@Example.com".into()),
            Owner::Team("platform".into())
        ]
    );
    // Teams expanded, emails lowercased, sorted and de-duplicated.
    assert_eq!(
        payments.owner_pilots,
        ["lina@example.com", "omar@example.com", "sara@example.com"]
    );

    let src = policy.zone(&rp("src/main.rs")).unwrap();
    assert_eq!(src.owner_pilots, ["lina@example.com", "omar@example.com"]);

    // Owners may be listed on non-owned zones for review routing.
    let docs = policy.zone(&rp("payments/docs/x.md")).unwrap();
    assert_eq!(docs.kind, ZoneKind::Open);
    assert_eq!(docs.owner_pilots, ["docs@example.com"]);

    // No owners at all.
    let shared = policy.zone(&rp("payments/api/x")).unwrap();
    assert!(shared.owners.is_empty() && shared.owner_pilots.is_empty());
}

#[test]
fn a_team_without_members_loads_with_a_warning() {
    let dir = policy_dir(&[("territory.toml", TERRITORY)]);
    let loaded = load(dir.path(), CS).unwrap();
    assert_eq!(loaded.warnings.len(), 1, "{:?}", loaded.warnings);
    assert!(loaded.warnings[0].contains("team \"empty\""));
}

#[test]
fn sealed_zones_in_territory_are_reported_as_zones_not_as_sealed_globs() {
    let policy = load_ok(&[("territory.toml", TERRITORY)], CS);
    assert_eq!(policy.sealed_match(&rp("vendor/lib/a.c")), None);
    assert_eq!(
        policy.zone(&rp("vendor/lib/a.c")).unwrap().kind,
        ZoneKind::Sealed
    );
}

// ---------------------------------------------------------------------------------------------
// Case sensitivity
// ---------------------------------------------------------------------------------------------

#[test]
fn matching_is_case_insensitive_when_asked() {
    let files = [
        (
            "guards.toml",
            "sealed = [\"Secrets/**\"]\nprotected_branches = [\"Main\", \"Release/*\"]\n[destructive]\nban = [\"git push --force*\"]\n",
        ),
        (
            "territory.toml",
            "[[zone]]\npaths = [\"Payments/**\"]\nkind = \"shared\"\n",
        ),
    ];
    let cs = load_ok(&files, CS);
    let ci = load_ok(&files, CI);
    assert!(ci.case_insensitive() && !cs.case_insensitive());

    // Sealed globs, built-in ones included.
    for path in [
        "secrets/key",
        "SECRETS/KEY",
        ".GIT/config",
        ".Acs/policy/x",
        "App/.ENV",
    ] {
        assert!(ci.sealed_match(&rp(path)).is_some(), "ci {path}");
    }
    assert_eq!(cs.sealed_match(&rp("secrets/key")), None);
    assert_eq!(cs.sealed_match(&rp(".GIT/config")), None);
    assert_eq!(cs.sealed_match(&rp("Secrets/key")), Some("Secrets/**"));
    // The reported pattern is the one the user wrote.
    assert_eq!(ci.sealed_match(&rp("SECRETS/key")), Some("Secrets/**"));

    // Zones.
    assert_eq!(ci.zone(&rp("payments/a")).unwrap().pattern, "Payments/**");
    assert!(cs.zone(&rp("payments/a")).is_none());
    assert!(cs.zone(&rp("Payments/a")).is_some());

    // Branches.
    assert!(ci.is_protected_branch("main") && ci.is_protected_branch("RELEASE/1"));
    assert!(!cs.is_protected_branch("main") && cs.is_protected_branch("Main"));

    // Ban patterns are matched literally in both modes.
    for policy in [&cs, &ci] {
        assert!(policy.banned_match("git push --force").is_some());
        assert!(policy.banned_match("GIT push --force").is_none());
    }
}

// ---------------------------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------------------------

struct ErrorCase {
    name: &'static str,
    file: PolicyFile,
    content: &'static str,
    kind: ProblemKind,
    /// 1-based line and column; `None` when the position is not known.
    at: Option<(usize, usize)>,
    message: &'static str,
}

const ERROR_CASES: &[ErrorCase] = &[
    ErrorCase {
        name: "toml syntax error",
        file: PolicyFile::Guards,
        content: "sealed = [\"a\"\nprotected_branches = []\n",
        kind: ProblemKind::Syntax,
        at: Some((2, 1)),
        message: "",
    },
    ErrorCase {
        name: "bare word as a value",
        file: PolicyFile::Territory,
        content: "[[zone]]\npaths = [\"a\"]\nkind = open\n",
        kind: ProblemKind::Syntax,
        at: Some((3, 8)),
        message: "",
    },
    ErrorCase {
        name: "duplicate team",
        file: PolicyFile::Territory,
        content: "[teams]\na = [\"x@y.z\"]\nb = []\na = []\n",
        kind: ProblemKind::Syntax,
        at: Some((4, 1)),
        message: "duplicate key",
    },
    ErrorCase {
        name: "config without spec",
        file: PolicyFile::Config,
        content: "claim_ttl_secs = 60\n",
        kind: ProblemKind::Schema,
        at: Some((1, 1)),
        message: "missing field `spec`",
    },
    ErrorCase {
        name: "empty config file",
        file: PolicyFile::Config,
        content: "",
        kind: ProblemKind::Schema,
        at: Some((1, 1)),
        message: "missing field `spec`",
    },
    ErrorCase {
        name: "wrong spec version",
        file: PolicyFile::Config,
        content: "spec = \"acs/0.2\"\n",
        kind: ProblemKind::Schema,
        at: Some((1, 8)),
        message: "unsupported spec version",
    },
    ErrorCase {
        name: "unknown config key",
        file: PolicyFile::Config,
        content: "spec = \"acs/0.1\"\n\nclaim_ttl = 5\n",
        kind: ProblemKind::Schema,
        at: Some((3, 1)),
        message: "unknown field `claim_ttl`",
    },
    ErrorCase {
        name: "zero ttl",
        file: PolicyFile::Config,
        content: "spec = \"acs/0.1\"\nclaim_ttl_secs = 0\n",
        kind: ProblemKind::Schema,
        at: Some((2, 18)),
        message: "nonzero",
    },
    ErrorCase {
        name: "wrong type",
        file: PolicyFile::Guards,
        content: "sealed = \"a/**\"\n",
        kind: ProblemKind::Schema,
        at: Some((1, 10)),
        message: "invalid type: string",
    },
    ErrorCase {
        name: "unknown guards key",
        file: PolicyFile::Guards,
        content: "[destructive]\nbans = []\n",
        kind: ProblemKind::Schema,
        at: Some((2, 1)),
        message: "unknown field `bans`",
    },
    ErrorCase {
        name: "unknown zone kind",
        file: PolicyFile::Territory,
        content: "[[zone]]\npaths = [\"a\"]\nkind = \"private\"\n",
        kind: ProblemKind::Schema,
        at: Some((3, 8)),
        message: "unknown variant `private`",
    },
    ErrorCase {
        name: "zone without paths",
        file: PolicyFile::Territory,
        content: "[[zone]]\nkind = \"open\"\n",
        kind: ProblemKind::Schema,
        at: Some((1, 1)),
        message: "missing field `paths`",
    },
    ErrorCase {
        name: "invalid owner",
        file: PolicyFile::Territory,
        content: "[[zone]]\npaths = [\"a\"]\nkind = \"owned\"\nowners = [\"sara\"]\n",
        kind: ProblemKind::Schema,
        at: Some((4, 10)),
        message: "invalid owner",
    },
    ErrorCase {
        name: "invalid sealed glob",
        file: PolicyFile::Guards,
        content: "sealed = [\n  \"ok/**\",\n  \"bad/[\",\n]\n",
        kind: ProblemKind::InvalidGlob,
        at: Some((3, 3)),
        message: "never closed",
    },
    ErrorCase {
        name: "sealed glob with leading slash",
        file: PolicyFile::Guards,
        content: "sealed = [\"/etc/**\"]\n",
        kind: ProblemKind::InvalidGlob,
        at: Some((1, 11)),
        message: "starts with \"/\"",
    },
    ErrorCase {
        name: "invalid protected branch glob",
        file: PolicyFile::Guards,
        content: "protected_branches = [\"main\", \"rel{ease\"]\n",
        kind: ProblemKind::InvalidGlob,
        at: Some((1, 31)),
        message: "never closed",
    },
    ErrorCase {
        name: "empty ban pattern",
        file: PolicyFile::Guards,
        content: "[destructive]\nban = [\"git push --force*\", \"  \"]\n",
        kind: ProblemKind::EmptyBan,
        at: Some((2, 29)),
        message: "empty",
    },
    ErrorCase {
        name: "ban pattern matching everything",
        file: PolicyFile::Guards,
        content: "[destructive]\nban = [\"*\"]\n",
        kind: ProblemKind::BanTooBroad,
        at: Some((2, 8)),
        message: "every command",
    },
    ErrorCase {
        name: "invalid zone glob",
        file: PolicyFile::Territory,
        content: "[[zone]]\npaths = [\"src/**\", \"a**b\"]\nkind = \"open\"\n",
        kind: ProblemKind::InvalidGlob,
        at: Some((2, 20)),
        message: "whole path segment",
    },
    ErrorCase {
        name: "zone with empty paths",
        file: PolicyFile::Territory,
        content: "[[zone]]\npaths = [\"a\"]\nkind = \"open\"\n\n[[zone]]\npaths = []\nkind = \"open\"\n",
        kind: ProblemKind::EmptyZonePaths,
        at: Some((6, 9)),
        message: "no paths",
    },
    ErrorCase {
        name: "owned zone without owners",
        file: PolicyFile::Territory,
        content: "[[zone]]\npaths = [\"a/**\"]\nkind = \"owned\"\n",
        kind: ProblemKind::OwnedWithoutOwners,
        at: Some((3, 8)),
        message: "`owned` but has no `owners`",
    },
    ErrorCase {
        name: "undefined team",
        file: PolicyFile::Territory,
        content: "[[zone]]\npaths = [\"a/**\"]\nkind = \"owned\"\nowners = [\"x@y.z\", \"team:ghost\"]\n\n[teams]\nreal = [\"x@y.z\"]\n",
        kind: ProblemKind::UndefinedTeam,
        at: Some((4, 20)),
        message: "\"team:ghost\" refers to a team that is not defined (defined teams: real)",
    },
    ErrorCase {
        name: "undefined team with no teams table",
        file: PolicyFile::Territory,
        content: "[[zone]]\npaths = [\"a/**\"]\nkind = \"shared\"\nowners = [\"team:ghost\"]\n",
        kind: ProblemKind::UndefinedTeam,
        at: Some((4, 11)),
        message: "no teams are defined",
    },
    ErrorCase {
        name: "team names differing in case",
        file: PolicyFile::Territory,
        content: "[teams]\nPlatform = [\"a@b.c\"]\nplatform = [\"a@b.c\"]\n",
        kind: ProblemKind::DuplicateTeam,
        at: Some((3, 1)),
        message: "duplicates team \"Platform\"",
    },
    ErrorCase {
        name: "unusable team name",
        file: PolicyFile::Territory,
        content: "[teams]\n\"my team\" = [\"a@b.c\"]\n",
        kind: ProblemKind::InvalidTeamName,
        at: Some((2, 1)),
        message: "cannot be referenced",
    },
    ErrorCase {
        name: "team member that is not an email",
        file: PolicyFile::Territory,
        content: "[teams]\nplatform = [\"a@b.c\", \"omar\"]\n",
        kind: ProblemKind::InvalidTeamMember,
        at: Some((2, 22)),
        message: "\"omar\" is not an email address",
    },
    ErrorCase {
        name: "empty target branch",
        file: PolicyFile::Config,
        content: "spec = \"acs/0.1\"\ndefault_target_branch = \" \"\n",
        kind: ProblemKind::InvalidValue,
        at: Some((2, 25)),
        message: "empty",
    },
];

#[test]
fn invalid_files_report_kind_position_message_and_fix() {
    let mut failures = Vec::new();
    for case in ERROR_CASES {
        let dir = policy_dir(&[(case.file.file_name(), case.content)]);
        let Err(err) = load(dir.path(), CS) else {
            failures.push(format!("{}: loaded without error", case.name));
            continue;
        };
        let problems = err.problems();
        let [problem] = problems else {
            failures.push(format!("{}: expected one problem: {err}", case.name));
            continue;
        };
        let position = problem.line.zip(problem.column);
        let text = err.to_string();
        let location = match case.at {
            Some((line, col)) => format!("{}:{line}:{col}: ", problem.path.display()),
            None => format!("{}: ", problem.path.display()),
        };
        let checks = [
            (problem.file == case.file, "file"),
            (problem.kind == case.kind, "kind"),
            (position == case.at, "position"),
            (problem.message.contains(case.message), "message"),
            (!problem.fix.is_empty(), "fix"),
            (
                problem.path == dir.path().join(case.file.file_name()),
                "path",
            ),
            // The rendered error leads with `path:line:col` and carries the fix.
            (text.contains(&location), "rendered location"),
            (text.contains("(fix: "), "rendered fix"),
        ];
        for (ok, what) in checks {
            if !ok {
                failures.push(format!("{}: wrong {what} ({position:?}): {err}", case.name));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn every_problem_kind_is_covered() {
    use ProblemKind::*;
    for kind in [
        Syntax,
        Schema,
        InvalidValue,
        InvalidGlob,
        EmptyZonePaths,
        OwnedWithoutOwners,
        UndefinedTeam,
        DuplicateTeam,
        InvalidTeamName,
        InvalidTeamMember,
        EmptyBan,
        BanTooBroad,
    ] {
        assert!(ERROR_CASES.iter().any(|c| c.kind == kind), "{kind:?}");
    }
}

#[test]
fn all_problems_of_all_files_are_reported_together_in_file_order() {
    let dir = policy_dir(&[
        (
            "territory.toml",
            "[[zone]]\npaths = [\"a[\"]\nkind = \"owned\"\n",
        ),
        ("guards.toml", "sealed = [\"b[\", \"/c\"]\n"),
        (
            "config.toml",
            "spec = \"acs/0.1\"\ndefault_target_branch = \"\"\n",
        ),
    ]);
    let err = load(dir.path(), CS).unwrap_err();
    let summary: Vec<_> = err
        .problems()
        .iter()
        .map(|p| (p.file, p.kind, p.line))
        .collect();
    assert_eq!(
        summary,
        [
            (PolicyFile::Config, ProblemKind::InvalidValue, Some(2)),
            (PolicyFile::Guards, ProblemKind::InvalidGlob, Some(1)),
            (PolicyFile::Guards, ProblemKind::InvalidGlob, Some(1)),
            (PolicyFile::Territory, ProblemKind::InvalidGlob, Some(2)),
            (
                PolicyFile::Territory,
                ProblemKind::OwnedWithoutOwners,
                Some(3)
            ),
        ]
    );
    let text = err.to_string();
    assert!(text.starts_with("invalid policy, 5 problems:"), "{text}");
    // The guards entries are told apart by column.
    assert!(err.problems()[1].column < err.problems()[2].column);
}

#[test]
fn syntax_errors_in_one_file_stop_semantic_checks_of_all() {
    // Semantic validation needs all three files to parse; the syntax error is reported alone.
    let dir = policy_dir(&[
        ("guards.toml", "sealed = [\"b[\"]\n"),
        ("config.toml", "spec = \n"),
    ]);
    let err = load(dir.path(), CS).unwrap_err();
    assert_eq!(err.problems().len(), 1);
    assert_eq!(err.problems()[0].kind, ProblemKind::Syntax);
}

#[test]
fn non_utf8_and_bom() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("guards.toml"), b"sealed = [\"\xff\"]").unwrap();
    let err = load(dir.path(), CS).unwrap_err();
    assert_eq!(err.problems()[0].kind, ProblemKind::Encoding);
    assert_eq!(err.problems()[0].line, None);

    // A byte-order mark is tolerated and does not shift columns.
    fs::write(
        dir.path().join("guards.toml"),
        "\u{feff}sealed = [\"a[\"]\n",
    )
    .unwrap();
    let err = load(dir.path(), CS).unwrap_err();
    assert_eq!(
        err.problems()[0].line.zip(err.problems()[0].column),
        Some((1, 11))
    );
    fs::write(dir.path().join("guards.toml"), "\u{feff}sealed = [\"a\"]\n").unwrap();
    assert!(load(dir.path(), CS).is_ok());
}

#[test]
fn columns_count_characters() {
    let dir = policy_dir(&[("guards.toml", "sealed = [\"é\", \"b[\"]\n")]);
    let err = load(dir.path(), CS).unwrap_err();
    assert_eq!(err.problems()[0].column, Some(16));
}

#[test]
fn a_policy_file_that_is_a_directory_is_an_io_error() {
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir(dir.path().join("guards.toml")).unwrap();
    let err = load(dir.path(), CS).unwrap_err();
    assert!(matches!(err, PolicyError::Io { .. }), "{err}");
    assert!(err.to_string().contains("guards.toml"), "{err}");
    assert!(err.problems().is_empty());
}

#[test]
fn compile_reports_semantic_problems_without_positions() {
    let mut territory = Territory::default();
    territory.zone.push(acs_core::Zone {
        paths: vec!["a/".into()],
        kind: ZoneKind::Owned,
        owners: vec![Owner::Team("ghost".into())],
    });
    let err = CompiledPolicy::compile(Config::default(), Guards::default(), territory, false)
        .unwrap_err();
    let kinds: Vec<_> = err.problems().iter().map(|p| p.kind).collect();
    assert_eq!(
        kinds,
        [ProblemKind::InvalidGlob, ProblemKind::UndefinedTeam]
    );
    assert!(err.problems().iter().all(|p| p.line.is_none()));
}

// ---------------------------------------------------------------------------------------------
// Hash
// ---------------------------------------------------------------------------------------------

#[test]
fn policy_hash_covers_names_presence_and_content() {
    let hash = |files: &[(&str, &str)]| policy_hash(policy_dir(files).path()).unwrap();
    let none = hash(&[]);
    let again = hash(&[]);
    assert_eq!(none, again);
    let guards = hash(&[("guards.toml", "")]);
    let territory = hash(&[("territory.toml", "")]);
    let config = hash(&[("config.toml", "")]);
    let changed = hash(&[("guards.toml", "sealed = []")]);
    let all = [&none, &guards, &territory, &config, &changed];
    for (i, a) in all.iter().enumerate() {
        for b in &all[i + 1..] {
            assert_ne!(a, b);
        }
    }
    // Loading reports the same hash.
    let dir = policy_dir(&[("guards.toml", "sealed = []")]);
    assert_eq!(load(dir.path(), CS).unwrap().policy.hash(), &changed);
}

// ---------------------------------------------------------------------------------------------
// Cache and fallbacks
// ---------------------------------------------------------------------------------------------

fn cache_file(cache: &Path) -> std::path::PathBuf {
    cache.join(CACHE_FILE)
}

#[test]
fn cache_lifecycle_fresh_then_cache_then_fresh_again() {
    let policy = policy_dir(&[("guards.toml", "sealed = [\"x/**\"]\n")]);
    let cache = tempfile::tempdir().unwrap();

    let first = load_cached(policy.path(), cache.path(), CS);
    assert_eq!(first.origin, Origin::Fresh);
    assert!(first.warnings.is_empty(), "{:?}", first.warnings);
    assert!(cache_file(cache.path()).exists());
    assert_eq!(first.policy.sealed_match(&rp("x/y")), Some("x/**"));

    let second = load_cached(policy.path(), cache.path(), CS);
    assert_eq!(second.origin, Origin::Cache);
    assert_eq!(second.policy.hash(), first.policy.hash());
    assert_eq!(second.policy.files(), first.policy.files());
    assert_eq!(second.policy.sealed_match(&rp("x/y")), Some("x/**"));

    // The cache serves either case-sensitivity setting.
    let third = load_cached(policy.path(), cache.path(), CI);
    assert_eq!(third.origin, Origin::Cache);
    assert!(third.policy.sealed_match(&rp("X/y")).is_some());

    // Editing a file invalidates the entry.
    fs::write(policy.path().join("guards.toml"), "sealed = [\"z/**\"]\n").unwrap();
    let fourth = load_cached(policy.path(), cache.path(), CS);
    assert_eq!(fourth.origin, Origin::Fresh);
    assert_eq!(fourth.policy.sealed_match(&rp("z/y")), Some("z/**"));
    assert_eq!(fourth.policy.sealed_match(&rp("x/y")), None);
    assert_eq!(
        load_cached(policy.path(), cache.path(), CS).origin,
        Origin::Cache
    );
}

#[test]
fn cache_directory_is_created_and_left_without_temporary_files() {
    let policy = policy_dir(&[]);
    let root = tempfile::tempdir().unwrap();
    let cache = root.path().join("nested").join("cache");
    let outcome = load_cached(policy.path(), &cache, CS);
    assert_eq!(outcome.origin, Origin::Fresh);
    let names: Vec<_> = fs::read_dir(&cache)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(names, [CACHE_FILE]);
}

#[cfg(unix)]
#[test]
fn cache_file_is_private() {
    use std::os::unix::fs::PermissionsExt;
    let policy = policy_dir(&[]);
    let cache = tempfile::tempdir().unwrap();
    let _ = load_cached(policy.path(), cache.path(), CS);
    let mode = fs::metadata(cache_file(cache.path()))
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(mode & 0o077, 0, "mode {mode:o}");
}

#[test]
fn invalid_policy_falls_back_to_the_last_valid_one() {
    let policy = policy_dir(&[("guards.toml", "sealed = [\"x/**\"]\n")]);
    let cache = tempfile::tempdir().unwrap();
    let good = load_cached(policy.path(), cache.path(), CS);
    assert_eq!(good.origin, Origin::Fresh);

    fs::write(policy.path().join("guards.toml"), "sealed = [\"x/**\"\n").unwrap();
    let outcome = load_cached(policy.path(), cache.path(), CS);
    assert_eq!(outcome.origin, Origin::LastValid);
    assert_eq!(outcome.policy.hash(), good.policy.hash());
    assert_eq!(outcome.policy.sealed_match(&rp("x/y")), Some("x/**"));
    assert_eq!(outcome.warnings.len(), 1, "{:?}", outcome.warnings);
    let warning = &outcome.warnings[0];
    assert!(warning.contains("guards.toml:1:17"), "{warning}");
    assert!(warning.contains("last valid policy"), "{warning}");

    // The invalid file was not cached: a repeat still falls back, and fixing the file recovers.
    assert_eq!(
        load_cached(policy.path(), cache.path(), CS).origin,
        Origin::LastValid
    );
    fs::write(policy.path().join("guards.toml"), "sealed = [\"y/**\"]\n").unwrap();
    let fixed = load_cached(policy.path(), cache.path(), CS);
    assert_eq!(fixed.origin, Origin::Fresh);
    assert!(fixed.warnings.is_empty());
}

#[test]
fn semantic_errors_also_fall_back() {
    let policy = policy_dir(&[(
        "territory.toml",
        "[[zone]]\npaths = [\"a/**\"]\nkind = \"open\"\n",
    )]);
    let cache = tempfile::tempdir().unwrap();
    let _ = load_cached(policy.path(), cache.path(), CS);
    fs::write(
        policy.path().join("territory.toml"),
        "[[zone]]\npaths = [\"a/**\"]\nkind = \"owned\"\nowners = [\"team:ghost\"]\n",
    )
    .unwrap();
    let outcome = load_cached(policy.path(), cache.path(), CS);
    assert_eq!(outcome.origin, Origin::LastValid);
    assert_eq!(
        outcome.policy.zone(&rp("a/b")).unwrap().kind,
        ZoneKind::Open
    );
    assert!(
        outcome.warnings[0].contains("territory.toml:4:11"),
        "{:?}",
        outcome.warnings
    );
}

#[test]
fn invalid_policy_without_a_cache_uses_the_built_in_defaults() {
    let policy = policy_dir(&[
        ("guards.toml", "sealed = [\"x/**\"]\n"),
        ("config.toml", "oops"),
    ]);
    let cache = tempfile::tempdir().unwrap();
    let outcome = load_cached(policy.path(), cache.path(), CS);
    assert_eq!(outcome.origin, Origin::Defaults);
    assert_eq!(outcome.policy.files(), &acs_policy::PolicyFiles::builtin());
    assert_eq!(
        outcome.policy.sealed_match(&rp("x/y")),
        None,
        "defaults only"
    );
    assert!(outcome.policy.sealed_match(&rp(".acs/x")).is_some());
    assert_eq!(outcome.warnings.len(), 1);
    assert!(
        outcome.warnings[0].contains("config.toml"),
        "{:?}",
        outcome.warnings
    );
    assert!(outcome.warnings[0].contains("built-in default guards"));
    assert!(
        !cache_file(cache.path()).exists(),
        "defaults are not cached"
    );
}

#[test]
fn unreadable_policy_file_falls_back_too() {
    let policy = tempfile::tempdir().unwrap();
    fs::create_dir(policy.path().join("territory.toml")).unwrap();
    let cache = tempfile::tempdir().unwrap();
    let outcome = load_cached(policy.path(), cache.path(), CS);
    assert_eq!(outcome.origin, Origin::Defaults);
    assert!(
        outcome.warnings[0].contains("cannot read"),
        "{:?}",
        outcome.warnings
    );
}

#[test]
fn corrupt_or_foreign_cache_is_ignored() {
    let policy = policy_dir(&[("guards.toml", "sealed = [\"x/**\"]\n")]);
    let cache = tempfile::tempdir().unwrap();
    let path = cache_file(cache.path());

    for garbage in [&b"not json"[..], b"{}", b"", b"\xff\xfe"] {
        fs::write(&path, garbage).unwrap();
        let outcome = load_cached(policy.path(), cache.path(), CS);
        assert_eq!(outcome.origin, Origin::Fresh, "{garbage:?}");
        assert_eq!(
            outcome.warnings.len(),
            1,
            "{garbage:?}: {:?}",
            outcome.warnings
        );
        assert!(
            outcome.warnings[0].contains("ignoring"),
            "{:?}",
            outcome.warnings
        );
        // Rebuilt by the load.
        assert_eq!(
            load_cached(policy.path(), cache.path(), CS).origin,
            Origin::Cache
        );
    }

    // Valid JSON whose policy is no longer valid (for example written by a build with a different
    // dialect) is recompiled from the files, not trusted.
    let entry = fs::read_to_string(&path).unwrap();
    let tampered = entry.replace("x/**", "x/[");
    assert_ne!(entry, tampered);
    fs::write(&path, tampered).unwrap();
    assert_eq!(
        load_cached(policy.path(), cache.path(), CS).origin,
        Origin::Fresh
    );

    // A different format or version is silently stale.
    let entry = fs::read_to_string(&path).unwrap();
    fs::write(&path, entry.replace("\"format\":2", "\"format\":99")).unwrap();
    let outcome = load_cached(policy.path(), cache.path(), CS);
    assert_eq!(outcome.origin, Origin::Fresh);
    assert!(outcome.warnings.is_empty());
}

#[test]
fn a_corrupt_cache_does_not_hide_an_invalid_policy_behind_stale_data() {
    let policy = policy_dir(&[("guards.toml", "sealed = [\n")]);
    let cache = tempfile::tempdir().unwrap();
    fs::write(cache_file(cache.path()), "garbage").unwrap();
    let outcome = load_cached(policy.path(), cache.path(), CS);
    assert_eq!(outcome.origin, Origin::Defaults);
    assert_eq!(outcome.warnings.len(), 2, "{:?}", outcome.warnings);
}

#[test]
fn an_unwritable_cache_is_a_warning_not_an_error() {
    let policy = policy_dir(&[]);
    let root = tempfile::tempdir().unwrap();
    // A file where the cache directory should be.
    let blocker = root.path().join("cache");
    fs::write(&blocker, "x").unwrap();
    let outcome = load_cached(policy.path(), &blocker, CS);
    assert_eq!(outcome.origin, Origin::Fresh);
    assert!(
        outcome
            .warnings
            .iter()
            .any(|w| w.contains("could not write the policy cache")),
        "{:?}",
        outcome.warnings
    );
}

// ---------------------------------------------------------------------------------------------
// Performance
// ---------------------------------------------------------------------------------------------

/// A policy of realistic size: 20 sealed globs, 30 zones, 5 teams.
fn realistic_policy() -> (String, String) {
    let mut guards = String::from("sealed = [\n");
    for i in 0..14 {
        guards.push_str(&format!("  \"services/svc{i}/secrets/**\",\n"));
    }
    for glob in [
        "**/*.pem",
        "**/id_rsa*",
        "deploy/prod/**",
        "infra/terraform/*.tfstate",
        "{keys,certs}/**",
        "**/credentials.json",
    ] {
        guards.push_str(&format!("  \"{glob}\",\n"));
    }
    guards.push_str("]\nprotected_branches = [\"main\", \"release/*\", \"hotfix/**\"]\n");
    guards.push_str("[destructive]\nmax_files_deleted_outside_claims = 2\nban = [\"git push --force*\", \"git reset --hard*\", \"git clean -f*\", \"rm -rf /*\", \"*--no-verify*\"]\n");

    let mut territory = String::new();
    for i in 0..30 {
        let (kind, owners) = match i % 4 {
            0 => ("owned", "owners = [\"team:t0\", \"lead@example.com\"]\n"),
            1 => ("shared", ""),
            2 => ("open", "owners = [\"team:t1\"]\n"),
            _ => ("owned", "owners = [\"team:t2\"]\n"),
        };
        territory.push_str(&format!(
            "[[zone]]\npaths = [\"services/svc{i}/**\", \"libs/lib{i}/src/**/*.rs\"]\nkind = \"{kind}\"\n{owners}\n"
        ));
    }
    territory.push_str("[teams]\n");
    for t in 0..5 {
        territory.push_str(&format!(
            "t{t} = [\"a{t}@example.com\", \"b{t}@example.com\", \"c{t}@example.com\", \"d{t}@example.com\"]\n"
        ));
    }
    (guards, territory)
}

#[test]
fn loading_a_cached_policy_is_fast() {
    let (guards, territory) = realistic_policy();
    let policy = policy_dir(&[
        ("guards.toml", &guards),
        ("territory.toml", &territory),
        ("config.toml", "spec = \"acs/0.1\"\n"),
    ]);
    let cache = tempfile::tempdir().unwrap();
    let first = load_cached(policy.path(), cache.path(), CS);
    assert_eq!(first.origin, Origin::Fresh, "{:?}", first.warnings);
    assert_eq!(
        first.policy.files().guards.sealed.len(),
        20 + DEFAULT_SEALED.len()
    );
    assert_eq!(first.policy.files().territory.zone.len(), 30);

    const RUNS: u32 = 200;
    let mut best = std::time::Duration::MAX;
    let mut total = std::time::Duration::ZERO;
    for _ in 0..RUNS {
        let start = Instant::now();
        let outcome = load_cached(policy.path(), cache.path(), CS);
        let elapsed = start.elapsed();
        assert_eq!(outcome.origin, Origin::Cache);
        // Use the result so nothing is optimized away.
        assert!(outcome.policy.zone(&rp("services/svc3/a.rs")).is_some());
        best = best.min(elapsed);
        total += elapsed;
    }
    let mean = total / RUNS;
    println!("cached policy load (20 sealed, 30 zones, 5 teams): mean {mean:?}, best {best:?}");
    // Release builds must meet the 1 ms budget; debug builds (and slow CI disks) get slack.
    let budget = if cfg!(debug_assertions) {
        std::time::Duration::from_millis(25)
    } else {
        std::time::Duration::from_millis(1)
    };
    assert!(mean < budget, "mean {mean:?} exceeds {budget:?}");
}
