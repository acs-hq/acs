//! Shared fixtures for the engine tests: a small policy with a tiny glob matcher, and builders for
//! gate inputs.

#![allow(dead_code, clippy::unwrap_used, clippy::expect_used)]

use acs_core::gate::{
    ClaimSource, ClaimsSnapshot, Confidence, Expansion, GateInput, GateState, GitInvocation,
    HeldClaim, OutsideSeal, OverrideView, PathLocation, PolicyView, ResolvedPath, ShellAnalysis,
    ShellTarget, SimpleCommand, Target, TargetOp, ZoneMatch,
};
use acs_core::{
    Claim, ClaimOrigin, ClaimScope, Config, DEFAULT_SEALED, Destructive, Notice, NoticeKind,
    OverrideId, Owner, Phase, RepoPath, Seat, SpecVersion, Timestamp, Tool, ToolEvent, ToolKind,
    ZoneKind,
};

/// The time of every fixture.
pub fn now() -> Timestamp {
    Timestamp::from_second(1_800_000_000).unwrap()
}

/// `now` minus `secs`.
pub fn ago(secs: i64) -> Timestamp {
    Timestamp::from_second(1_800_000_000 - secs).unwrap()
}

pub fn rp(path: &str) -> RepoPath {
    path.parse().unwrap()
}

// ---------------------------------------------------------------------------------------------
// Policy
// ---------------------------------------------------------------------------------------------

/// Matches `/`-separated `path` against `pattern`: `*` and `?` within a segment, `**` as a whole
/// segment for zero or more segments. (`[]` and `{}` are not needed by the tests.)
pub fn glob(pattern: &str, path: &str) -> bool {
    let pattern: Vec<&str> = pattern.split('/').collect();
    let path: Vec<&str> = path.split('/').collect();
    glob_segments(&pattern, &path)
}

fn glob_segments(pattern: &[&str], path: &[&str]) -> bool {
    match pattern.split_first() {
        None => path.is_empty(),
        Some((&"**", rest)) => (0..=path.len()).any(|skip| glob_segments(rest, &path[skip..])),
        Some((segment, rest)) => path.split_first().is_some_and(|(head, tail)| {
            wildcard(segment, head, false) && glob_segments(rest, tail)
        }),
    }
}

/// `*` matches any run of characters (including `/` when `any` is set, otherwise not), `?` one.
pub fn wildcard(pattern: &str, text: &str, any: bool) -> bool {
    fn go(p: &[char], t: &[char], any: bool) -> bool {
        match p.split_first() {
            None => t.is_empty(),
            Some(('*', rest)) => (0..=t.len())
                .take_while(|&i| i == 0 || any || t[i - 1] != '/')
                .any(|i| go(rest, &t[i..], any)),
            Some(('?', rest)) => t.split_first().is_some_and(|(_, tail)| go(rest, tail, any)),
            Some((c, rest)) => t
                .split_first()
                .is_some_and(|(h, tail)| h == c && go(rest, tail, any)),
        }
    }
    let p: Vec<char> = pattern.chars().collect();
    let t: Vec<char> = text.chars().collect();
    go(&p, &t, any)
}

pub struct FakeZone {
    pub pattern: String,
    pub kind: ZoneKind,
    pub owners: Vec<Owner>,
    pub pilots: Vec<String>,
}

/// A policy with the default sealed globs, no zones, no protected branches and no bans.
pub struct FakePolicy {
    pub config: Config,
    pub destructive: Destructive,
    pub sealed: Vec<String>,
    pub zones: Vec<FakeZone>,
    pub protected: Vec<String>,
    pub outside: bool,
}

impl Default for FakePolicy {
    fn default() -> Self {
        Self {
            config: Config::default(),
            destructive: Destructive::default(),
            sealed: DEFAULT_SEALED.iter().map(|s| (*s).to_owned()).collect(),
            zones: Vec::new(),
            protected: Vec::new(),
            outside: false,
        }
    }
}

impl FakePolicy {
    pub fn zone_of(mut self, pattern: &str, kind: ZoneKind) -> Self {
        self.zones.push(FakeZone {
            pattern: pattern.to_owned(),
            kind,
            owners: vec![Owner::Pilot("sara@example.com".into())],
            pilots: vec!["sara@example.com".into()],
        });
        self
    }

    /// An `owned` zone whose only owner is `pilot`.
    pub fn owned_by(mut self, pattern: &str, pilot: &str) -> Self {
        self.zones.push(FakeZone {
            pattern: pattern.to_owned(),
            kind: ZoneKind::Owned,
            owners: vec![Owner::Pilot(pilot.into())],
            pilots: vec![pilot.into()],
        });
        self
    }

    /// `protect_outside_repo = true`.
    pub fn guarding_outside(mut self) -> Self {
        self.outside = true;
        self
    }

    pub fn protecting(mut self, pattern: &str) -> Self {
        self.protected.push(pattern.to_owned());
        self
    }

    pub fn banning(mut self, pattern: &str) -> Self {
        self.destructive.ban.push(pattern.to_owned());
        self
    }

    pub fn budgets(mut self, outside_claims: u32, per_command: u32) -> Self {
        self.destructive.max_files_deleted_outside_claims = outside_claims;
        self.destructive.max_files_deleted_per_command = per_command;
        self
    }
}

impl PolicyView for FakePolicy {
    fn config(&self) -> &Config {
        &self.config
    }

    fn destructive(&self) -> &Destructive {
        &self.destructive
    }

    fn sealed_match(&self, path: &RepoPath) -> Option<&str> {
        self.sealed
            .iter()
            .find(|p| glob(p, path.as_str()))
            .map(String::as_str)
    }

    fn zone(&self, path: &RepoPath) -> Option<ZoneMatch<'_>> {
        // The most specific match is the longest pattern.
        self.zones
            .iter()
            .filter(|z| glob(&z.pattern, path.as_str()))
            .max_by_key(|z| z.pattern.len())
            .map(|z| ZoneMatch {
                kind: z.kind,
                pattern: &z.pattern,
                owners: &z.owners,
                owner_pilots: &z.pilots,
            })
    }

    fn is_protected_branch(&self, branch: &str) -> bool {
        self.protected.iter().any(|p| glob(p, branch))
    }

    fn protect_outside_repo(&self) -> bool {
        self.outside
    }

    fn banned_match(&self, command: &str) -> Option<&str> {
        self.destructive
            .ban
            .iter()
            .find(|p| wildcard(p, command, true))
            .map(String::as_str)
    }
}

// ---------------------------------------------------------------------------------------------
// Inputs
// ---------------------------------------------------------------------------------------------

pub fn seat(id: &str, pilot: &str, branch: &str, target: &str) -> Seat {
    Seat {
        seat_id: id.parse().unwrap(),
        pilot: pilot.into(),
        harness: "claude-code".into(),
        harness_session_id: format!("session-{id}"),
        worktree: "/work/repo".into(),
        branch: Some(branch.into()),
        target_branch: target.into(),
        started_at: ago(3600),
        last_seen_at: now(),
    }
}

pub fn event(kind: ToolKind, phase: Phase) -> ToolEvent {
    ToolEvent {
        spec: SpecVersion,
        harness: "canonical".into(),
        harness_session_id: "session-seat_me".into(),
        phase,
        cwd: "/work/repo".into(),
        tool: Some(Tool {
            name: "T".into(),
            kind,
        }),
        paths: Vec::new(),
        edit: None,
        command: None,
        result: None,
        prompt: None,
        raw: serde_json::Value::Null,
    }
}

pub fn in_repo(path: &str, op: TargetOp) -> Target {
    Target {
        path: ResolvedPath {
            raw: path.into(),
            location: PathLocation::InRepo(rp(path)),
            via_link: false,
            linked_into_repo: Vec::new(),
        },
        op,
        recursive: false,
        expansion: None,
    }
}

pub fn write(path: &str) -> Target {
    in_repo(path, TargetOp::Write)
}

pub fn delete(path: &str) -> Target {
    in_repo(path, TargetOp::Delete)
}

pub fn read(path: &str) -> Target {
    in_repo(path, TargetOp::Read)
}

pub fn outside(raw: &str, op: TargetOp) -> Target {
    Target {
        path: ResolvedPath {
            raw: raw.into(),
            location: PathLocation::Outside(raw.into()),
            via_link: false,
            linked_into_repo: Vec::new(),
        },
        op,
        recursive: false,
        expansion: None,
    }
}

pub trait TargetExt: Sized {
    fn recursive(self, files: &[&str]) -> Self;
    fn expanded(self, files: &[&str], truncated: bool) -> Self;
    fn linked(self, paths: &[&str]) -> Self;
}

impl TargetExt for Target {
    fn recursive(mut self, files: &[&str]) -> Self {
        self.recursive = true;
        self.expansion = Some(Expansion {
            files: files.iter().map(|f| rp(f)).collect(),
            truncated: false,
        });
        self
    }

    fn expanded(mut self, files: &[&str], truncated: bool) -> Self {
        self.expansion = Some(Expansion {
            files: files.iter().map(|f| rp(f)).collect(),
            truncated,
        });
        self
    }

    fn linked(mut self, paths: &[&str]) -> Self {
        self.path.via_link = true;
        self.path.linked_into_repo = paths.iter().map(|p| rp(p)).collect();
        self
    }
}

/// A claim held by `seat` on `path`.
pub fn held(seat: &str, pilot: &str, path: &str, source: ClaimSource, idle_secs: i64) -> HeldClaim {
    held_scope(
        seat,
        pilot,
        ClaimScope::File { file: rp(path) },
        "main",
        source,
        idle_secs,
    )
}

pub fn held_scope(
    seat: &str,
    pilot: &str,
    scope: ClaimScope,
    target_branch: &str,
    source: ClaimSource,
    idle_secs: i64,
) -> HeldClaim {
    HeldClaim {
        claim: Claim {
            claim_id: "clm_1".parse().unwrap(),
            seat_id: seat.parse().unwrap(),
            pilot: pilot.into(),
            scope,
            target_branch: target_branch.into(),
            acquired_at: ago(idle_secs + 60),
            heartbeat_at: ago(idle_secs),
            ttl_secs: 900,
            origin: ClaimOrigin::Agent,
        },
        source,
        holder_harness: (source == ClaimSource::Local).then(|| "claude-code".to_owned()),
        holder_last_seen_at: None,
    }
}

pub fn notice(id: &str, seat: &str, body: &str) -> Notice {
    Notice {
        notice_id: id.parse().unwrap(),
        kind: NoticeKind::ClaimReleased,
        target_seat: seat.parse().unwrap(),
        body: body.into(),
        created_at: ago(30),
        delivered_at: None,
    }
}

pub fn command(words: &[&str], classes: &[acs_core::gate::CommandClass]) -> SimpleCommand {
    SimpleCommand {
        words: words.iter().map(|w| (*w).to_owned()).collect(),
        classes: classes.to_vec(),
        targets: Vec::new(),
        targets_known: true,
        git: None,
    }
}

pub fn shell_target(path: &str, op: TargetOp, confidence: Confidence) -> ShellTarget {
    ShellTarget {
        path: path.into(),
        op,
        recursive: false,
        confidence,
    }
}

pub fn git_command(
    words: &[&str],
    subcommand: &str,
    rewrites_history: bool,
    force_push: bool,
    refs: &[&str],
) -> SimpleCommand {
    let mut cmd = command(words, &[acs_core::gate::CommandClass::GitHistoryRewrite]);
    cmd.git = Some(GitInvocation {
        subcommand: subcommand.into(),
        rewrites_history,
        force_push,
        creates_commit: false,
        refs: refs.iter().map(|r| (*r).to_owned()).collect(),
        ..GitInvocation::default()
    });
    cmd
}

pub fn analysis(commands: Vec<SimpleCommand>) -> ShellAnalysis {
    ShellAnalysis {
        commands,
        complete: true,
    }
}

/// Overrides as `(glob, id)` pairs, matched with [`glob`].
#[derive(Debug, Default)]
pub struct FakeOverrides(pub Vec<(String, OverrideId)>);

impl OverrideView for FakeOverrides {
    fn covering(&self, path: &RepoPath) -> Option<&OverrideId> {
        self.0
            .iter()
            .find(|(pattern, _)| glob(pattern, path.as_str()))
            .map(|(_, id)| id)
    }
}

/// Everything a `GateInput` borrows, owned in one place.
pub struct Fixture {
    pub event: ToolEvent,
    pub seat: Seat,
    pub origin: ClaimOrigin,
    pub targets: Vec<Target>,
    pub shell: Option<ShellAnalysis>,
    pub claims: ClaimsSnapshot,
    pub notices: Vec<Notice>,
    pub overrides: FakeOverrides,
    pub outside_sealed: Vec<OutsideSeal>,
    pub state: GateState,
}

impl Fixture {
    /// A pre-tool event of `kind` by `seat_me` (pilot omar, on `feature/x`, targeting `main`).
    pub fn new(kind: ToolKind) -> Self {
        Self {
            event: event(kind, Phase::PreTool),
            seat: seat("seat_me", "omar@example.com", "feature/x", "main"),
            origin: ClaimOrigin::Agent,
            targets: Vec::new(),
            shell: None,
            claims: ClaimsSnapshot::default(),
            notices: Vec::new(),
            overrides: FakeOverrides::default(),
            outside_sealed: Vec::new(),
            state: GateState::default(),
        }
    }

    pub fn edit(path: &str) -> Self {
        Self::new(ToolKind::Edit).with(write(path))
    }

    pub fn shell(commands: Vec<SimpleCommand>, targets: Vec<Target>) -> Self {
        let mut fixture = Self::new(ToolKind::Shell);
        fixture.shell = Some(analysis(commands));
        fixture.targets = targets;
        fixture
    }

    pub fn with(mut self, target: Target) -> Self {
        self.targets.push(target);
        self
    }

    pub fn claimed(mut self, claim: HeldClaim) -> Self {
        self.claims.claims.push(claim);
        self
    }

    /// A human acting through the CLI rather than an agent.
    pub fn human(mut self) -> Self {
        self.origin = ClaimOrigin::Human;
        self
    }

    /// An override in force for the seat.
    pub fn overridden(mut self, pattern: &str, id: &str) -> Self {
        self.overrides
            .0
            .push((pattern.to_owned(), id.parse().unwrap()));
        self
    }

    /// A location outside the repository that agents may not write.
    pub fn sealing_outside(mut self, path: &str, label: &str, home_relative: Option<&str>) -> Self {
        self.outside_sealed.push(OutsideSeal {
            path: path.to_owned(),
            label: label.to_owned(),
            home_relative: home_relative.map(str::to_owned),
        });
        self
    }

    pub fn gate_off(mut self) -> Self {
        self.state.gate_off = true;
        self
    }

    pub fn state_unavailable(mut self) -> Self {
        self.state.state_unavailable = true;
        self
    }

    pub fn input(&self) -> GateInput<'_> {
        GateInput {
            event: &self.event,
            seat: &self.seat,
            origin: self.origin,
            targets: &self.targets,
            shell: self.shell.as_ref(),
            claims: &self.claims,
            notices: &self.notices,
            overrides: &self.overrides,
            outside_sealed: &self.outside_sealed,
            state: self.state,
            now: now(),
        }
    }
}
