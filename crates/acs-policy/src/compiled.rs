//! The compiled policy: what the decision engine queries.

use std::collections::{BTreeMap, BTreeSet};

use acs_core::gate::{PolicyView, ZoneMatch};
use acs_core::{Config, ContentHash, Destructive, Guards, Owner, RepoPath, Territory, ZoneKind};

use crate::error::{PolicyError, PolicyFile, Problem, ProblemKind};
use crate::files::{PolicyFiles, effective_sealed};
use crate::glob::{Alt, BanPattern, Glob, with_segments};

/// Where in a policy file a problem was found; the parser turns it into a line and column.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum At {
    /// `default_target_branch`
    TargetBranch,
    /// The `sealed` entry with this text (the built-in defaults are not in the file, so an
    /// index would not line up).
    Sealed(String),
    /// `protected_branches[i]`
    Protected(usize),
    /// `destructive.ban[i]`
    Ban(usize),
    /// `zone[i].kind`
    ZoneKind(usize),
    /// `zone[i].paths`
    ZonePaths(usize),
    /// `zone[i].paths[j]`
    ZonePath(usize, usize),
    /// `zone[i].owners[j]`
    ZoneOwner(usize, usize),
    /// `teams.<name>`
    Team(String),
    /// `teams.<name>[j]`
    TeamMember(String, usize),
}

/// A semantic problem, not yet attached to a file position.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Found {
    pub file: PolicyFile,
    pub at: At,
    pub kind: ProblemKind,
    pub message: String,
    pub fix: String,
}

impl Found {
    fn new(
        file: PolicyFile,
        at: At,
        kind: ProblemKind,
        message: impl Into<String>,
        fix: impl Into<String>,
    ) -> Self {
        Self {
            file,
            at,
            kind,
            message: message.into(),
            fix: fix.into(),
        }
    }
}

/// One glob of one zone, ready for lookup.
#[derive(Debug)]
struct ZoneEntry {
    alt: Alt,
    zone: usize,
    path: usize,
}

/// A policy compiled for one repository: glob matchers, zone lookup tables and expanded team
/// ownership, behind the engine's [`PolicyView`].
///
/// # Zone specificity
///
/// When several zone globs match a path, the most specific wins. Each `{a,b}` alternative of a
/// glob is ranked on its own by, in order:
///
/// 1. more leading literal segments (`payments/api/**` beats `payments/**` beats `**/*.rs`),
/// 2. fewer `**` segments (`src/*.rs` beats `src/**/*.rs` beats `src/**`),
/// 3. more literal characters (`docs/**/*.md` beats `docs/**`),
/// 4. fewer wildcards (`ab*` beats `a*c*`).
///
/// A full tie goes to the zone listed later in `territory.toml` (and, within a zone, to the later
/// path), so a repository can always override an earlier entry by repeating the glob.
#[derive(Debug)]
pub struct CompiledPolicy {
    files: PolicyFiles,
    hash: ContentHash,
    case_insensitive: bool,
    sealed: Vec<Glob>,
    protected: Vec<Glob>,
    bans: Vec<BanPattern>,
    /// Zone globs, most specific first.
    zones: Vec<ZoneEntry>,
    /// Per zone: owners expanded to lowercased emails, sorted.
    pilots: Vec<Vec<String>>,
}

impl CompiledPolicy {
    /// Compiles a policy.
    ///
    /// `guards` is used as given, except that [`acs_core::DEFAULT_SEALED`] is always added to
    /// `sealed`: pass [`crate::builtin_guards`] or [`crate::resolve_guards`] output to get the
    /// built-in protected branches and bans. With `case_insensitive`, path and branch globs
    /// ignore case (for case-insensitive filesystems); ban patterns are always case-sensitive.
    ///
    /// The policy's [`hash`](Self::hash) is the digest of its canonical JSON form; policies
    /// loaded from disk carry the digest of the files instead.
    ///
    /// # Errors
    ///
    /// Returns [`PolicyError::Invalid`] (without line numbers) for every semantic problem:
    /// invalid globs, empty zones, undefined teams and so on.
    pub fn compile(
        config: Config,
        guards: Guards,
        territory: Territory,
        case_insensitive: bool,
    ) -> Result<Self, PolicyError> {
        let files = PolicyFiles {
            config,
            guards,
            territory,
        };
        let hash = ContentHash::of(&serde_json::to_vec(&files).unwrap_or_default());
        Self::build(files, hash, case_insensitive).map_err(|found| PolicyError::Invalid {
            problems: found.into_iter().map(unlocated).collect(),
        })
    }

    /// The policy in force when there are no policy files, compiled for the given case
    /// sensitivity.
    #[must_use]
    pub fn builtin(case_insensitive: bool) -> Self {
        let files = PolicyFiles::builtin();
        let hash = crate::load::hash_files(&[None, None, None]);
        match Self::build(files, hash, case_insensitive) {
            Ok(policy) => policy,
            // The built-in policy is a constant and a unit test compiles it for both settings.
            Err(found) => unreachable!("the built-in policy must be valid: {found:?}"),
        }
    }

    /// Compiles `files`, reporting every semantic problem found.
    pub(crate) fn build(
        mut files: PolicyFiles,
        hash: ContentHash,
        case_insensitive: bool,
    ) -> Result<Self, Vec<Found>> {
        let mut found = Vec::new();
        files.guards.sealed = effective_sealed(&files.guards.sealed);

        if files.config.default_target_branch.trim().is_empty() {
            found.push(Found::new(
                PolicyFile::Config,
                At::TargetBranch,
                ProblemKind::InvalidValue,
                "default_target_branch is empty",
                "name the branch work is integrated into, for example \"main\"",
            ));
        }

        let globs =
            |patterns: &[String], at: &dyn Fn(usize, &str) -> At, found: &mut Vec<Found>| {
                patterns
                    .iter()
                    .enumerate()
                    .filter_map(|(i, pattern)| {
                        compile_glob(
                            pattern,
                            case_insensitive,
                            PolicyFile::Guards,
                            at(i, pattern),
                            found,
                        )
                    })
                    .collect::<Vec<_>>()
            };
        let sealed = globs(
            &files.guards.sealed,
            &|_, glob| At::Sealed(glob.to_owned()),
            &mut found,
        );
        let protected = globs(
            &files.guards.protected_branches,
            &|i, _| At::Protected(i),
            &mut found,
        );
        let bans = compile_bans(&files.guards.destructive.ban, &mut found);

        let teams = check_teams(&files.territory.teams, &mut found);
        let mut zones = Vec::new();
        let mut pilots = Vec::new();
        for (zi, zone) in files.territory.zone.iter().enumerate() {
            if zone.paths.is_empty() {
                found.push(Found::new(
                    PolicyFile::Territory,
                    At::ZonePaths(zi),
                    ProblemKind::EmptyZonePaths,
                    "this zone has no paths, so it covers nothing",
                    "list at least one glob in `paths`, for example paths = [\"src/**\"], or delete the zone",
                ));
            }
            for (pi, pattern) in zone.paths.iter().enumerate() {
                let at = At::ZonePath(zi, pi);
                if let Some(glob) = compile_glob(
                    pattern,
                    case_insensitive,
                    PolicyFile::Territory,
                    at,
                    &mut found,
                ) {
                    zones.extend(glob.into_alts().into_iter().map(|alt| ZoneEntry {
                        alt,
                        zone: zi,
                        path: pi,
                    }));
                }
            }
            if zone.kind == ZoneKind::Owned && zone.owners.is_empty() {
                found.push(Found::new(
                    PolicyFile::Territory,
                    At::ZoneKind(zi),
                    ProblemKind::OwnedWithoutOwners,
                    "this zone is `owned` but has no `owners`, so nobody could write to it",
                    "add owners = [\"someone@example.com\", \"team:<name>\"], or change the kind",
                ));
            }
            pilots.push(expand_owners(zi, &zone.owners, &teams, &mut found));
        }
        if !found.is_empty() {
            return Err(found);
        }

        zones.sort_by(|a, b| {
            (b.alt.specificity(), b.zone, b.path).cmp(&(a.alt.specificity(), a.zone, a.path))
        });
        Ok(Self {
            files,
            hash,
            case_insensitive,
            sealed,
            protected,
            bans,
            zones,
            pilots,
        })
    }

    /// The validated, effective policy this was compiled from.
    #[must_use]
    pub fn files(&self) -> &PolicyFiles {
        &self.files
    }

    /// The policy hash: SHA-256 over the three policy files when loaded from disk (see
    /// [`policy_hash`](crate::policy_hash)).
    #[must_use]
    pub fn hash(&self) -> &ContentHash {
        &self.hash
    }

    /// Whether path and branch globs ignore case.
    #[must_use]
    pub fn case_insensitive(&self) -> bool {
        self.case_insensitive
    }

    /// Non-fatal observations about the policy, for the caller to show.
    pub(crate) fn warnings(&self) -> Vec<String> {
        self.files
            .territory
            .teams
            .iter()
            .filter(|(_, members)| members.is_empty())
            .map(|(name, _)| {
                format!(
                    "team \"{name}\" in territory.toml has no members; zones it owns cannot be written by any agent"
                )
            })
            .collect()
    }
}

impl PolicyView for CompiledPolicy {
    fn config(&self) -> &Config {
        &self.files.config
    }

    fn destructive(&self) -> &Destructive {
        &self.files.guards.destructive
    }

    fn sealed_match(&self, path: &RepoPath) -> Option<&str> {
        let index = with_segments(path.as_str(), self.case_insensitive, |segs| {
            self.sealed
                .iter()
                .position(|glob| glob.alts().iter().any(|alt| alt.is_match(segs)))
        })?;
        Some(self.files.guards.sealed[index].as_str())
    }

    fn zone(&self, path: &RepoPath) -> Option<ZoneMatch<'_>> {
        let entry = with_segments(path.as_str(), self.case_insensitive, |segs| {
            self.zones.iter().find(|entry| entry.alt.is_match(segs))
        })?;
        let zone = &self.files.territory.zone[entry.zone];
        Some(ZoneMatch {
            kind: zone.kind,
            pattern: zone.paths[entry.path].as_str(),
            owners: &zone.owners,
            owner_pilots: &self.pilots[entry.zone],
        })
    }

    fn is_protected_branch(&self, branch: &str) -> bool {
        with_segments(branch, self.case_insensitive, |segs| {
            self.protected
                .iter()
                .any(|glob| glob.alts().iter().any(|alt| alt.is_match(segs)))
        })
    }

    fn protect_outside_repo(&self) -> bool {
        self.files.guards.protect_outside_repo
    }

    fn banned_match(&self, command: &str) -> Option<&str> {
        let index = self.bans.iter().position(|ban| ban.is_match(command))?;
        Some(self.files.guards.destructive.ban[index].as_str())
    }
}

fn compile_glob(
    pattern: &str,
    case_insensitive: bool,
    file: PolicyFile,
    at: At,
    found: &mut Vec<Found>,
) -> Option<Glob> {
    match Glob::new(pattern, case_insensitive) {
        Ok(glob) => Some(glob),
        Err(err) => {
            found.push(Found::new(
                file,
                at,
                ProblemKind::InvalidGlob,
                format!("invalid glob {pattern:?}: {}", err.message),
                err.fix,
            ));
            None
        }
    }
}

fn compile_bans(patterns: &[String], found: &mut Vec<Found>) -> Vec<BanPattern> {
    let mut bans = Vec::new();
    for (i, pattern) in patterns.iter().enumerate() {
        let ban = BanPattern::new(pattern);
        let (kind, message, fix) = if pattern.trim().is_empty() {
            (
                ProblemKind::EmptyBan,
                "this ban pattern is empty".to_owned(),
                "write the command to ban, for example \"git push --force*\", or delete the entry",
            )
        } else if ban.matches_everything() {
            (
                ProblemKind::BanTooBroad,
                format!("the ban pattern {pattern:?} would deny every command"),
                "name the command to ban, for example \"git clean -f*\"",
            )
        } else {
            bans.push(ban);
            continue;
        };
        found.push(Found::new(
            PolicyFile::Guards,
            At::Ban(i),
            kind,
            message,
            fix,
        ));
    }
    bans
}

/// Validates `[teams]`, returning each team's members lowercased (only for valid teams).
fn check_teams(
    teams: &BTreeMap<String, Vec<String>>,
    found: &mut Vec<Found>,
) -> BTreeMap<String, Vec<String>> {
    let mut valid = BTreeMap::new();
    let mut seen: BTreeMap<String, &str> = BTreeMap::new();
    for (name, members) in teams {
        let name_ok = !name.is_empty()
            && name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'-'));
        if !name_ok {
            found.push(Found::new(
                PolicyFile::Territory,
                At::Team(name.clone()),
                ProblemKind::InvalidTeamName,
                format!("team name {name:?} cannot be referenced as an owner"),
                "use only letters, digits, `_`, `.` and `-`, so that `team:<name>` can name it",
            ));
        }
        if let Some(first) = seen.insert(name.to_lowercase(), name) {
            found.push(Found::new(
                PolicyFile::Territory,
                At::Team(name.clone()),
                ProblemKind::DuplicateTeam,
                format!("team {name:?} duplicates team {first:?} (names differing only in case)"),
                "merge the two teams, or rename one of them",
            ));
        }
        let mut emails = BTreeSet::new();
        for (j, member) in members.iter().enumerate() {
            if matches!(Owner::try_from(member.clone()), Ok(Owner::Pilot(_))) {
                emails.insert(member.to_lowercase());
            } else {
                found.push(Found::new(
                    PolicyFile::Territory,
                    At::TeamMember(name.clone(), j),
                    ProblemKind::InvalidTeamMember,
                    format!("team member {member:?} is not an email address"),
                    "list members by git email, for example \"sara@example.com\"",
                ));
            }
        }
        valid.insert(name.clone(), emails.into_iter().collect());
    }
    valid
}

/// Expands owners to sorted, de-duplicated, lowercased emails, reporting undefined teams.
fn expand_owners(
    zone: usize,
    owners: &[Owner],
    teams: &BTreeMap<String, Vec<String>>,
    found: &mut Vec<Found>,
) -> Vec<String> {
    let mut emails = BTreeSet::new();
    for (j, owner) in owners.iter().enumerate() {
        match owner {
            Owner::Pilot(email) => {
                emails.insert(email.to_lowercase());
            }
            Owner::Team(name) => {
                match teams.get(name) {
                    Some(members) => emails.extend(members.iter().cloned()),
                    None => {
                        let defined: Vec<&str> = teams.keys().map(String::as_str).collect();
                        let known = if defined.is_empty() {
                            "no teams are defined".to_owned()
                        } else {
                            format!("defined teams: {}", defined.join(", "))
                        };
                        found.push(Found::new(
                        PolicyFile::Territory,
                        At::ZoneOwner(zone, j),
                        ProblemKind::UndefinedTeam,
                        format!("owner \"team:{name}\" refers to a team that is not defined ({known})"),
                        format!("add `{name} = [\"member@example.com\"]` under [teams], or fix the name"),
                    ));
                    }
                }
            }
        }
    }
    emails.into_iter().collect()
}

/// A [`Problem`] without a position, for policies that did not come from a file.
fn unlocated(found: Found) -> Problem {
    Problem {
        path: found.file.file_name().into(),
        file: found.file,
        kind: found.kind,
        line: None,
        column: None,
        message: found.message,
        fix: found.fix,
    }
}
