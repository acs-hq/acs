//! Types of the committed policy files under `.acs/policy/`: `config.toml`, `guards.toml`,
//! `territory.toml`, and the front matter of lore entries.
//!
//! These types reject unknown fields so that a typo in a policy file is an error, not a silently
//! ignored setting.

use std::collections::BTreeMap;
use std::fmt;
use std::num::{NonZeroU32, NonZeroU64};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::error::ParseError;
use crate::ids::LoreId;
use crate::values::SpecVersion;

/// The default sealed globs: paths no agent may write, so that an agent cannot switch ACS off or
/// rewrite its own configuration. Adapters extend this list with their harness's hook and
/// settings files.
pub const DEFAULT_SEALED: &[&str] = &[
    ".git/**",
    ".acs/**",
    ".github/workflows/**",
    "**/.env*",
    "**/.claude/settings*.json",
    "**/.claude/hooks/**",
    "**/.agents/**/hooks*.json",
    "**/.agent/**/hooks*.json",
    "**/.agents/plugins/**",
    "**/.agent/plugins/**",
    "**/.agents/plugins.json",
    "**/.agent/plugins.json",
    ".husky/**",
];

/// The part of [`DEFAULT_SEALED`] that keeps ACS itself in place: its policy and state, git's
/// hooks and config, and the harness files that load the ACS hooks. These stay sealed when
/// `[features] guards` is off, so that switching a feature off never lets an agent switch ACS
/// off (or a feature back on, or another one off).
pub const SELF_PROTECTION_SEALED: &[&str] = &[
    ".git/**",
    ".acs/**",
    "**/.claude/settings*.json",
    "**/.claude/hooks/**",
    "**/.agents/**/hooks*.json",
    "**/.agent/**/hooks*.json",
    "**/.agents/plugins/**",
    "**/.agent/plugins/**",
    "**/.agents/plugins.json",
    "**/.agent/plugins.json",
    ".husky/**",
];

const DEFAULT_CLAIM_TTL_SECS: NonZeroU64 = non_zero(900);
const DEFAULT_SYNC_INTERVAL_SECS: NonZeroU64 = non_zero(30);
const DEFAULT_LORE_MAX_INJECT_CHARS: NonZeroU32 = match NonZeroU32::new(4_000) {
    Some(value) => value,
    None => panic!("constant must be non-zero"),
};
const DEFAULT_LORE_STALE_PERCENT: Percent = Percent(30);

const fn non_zero(value: u64) -> NonZeroU64 {
    match NonZeroU64::new(value) {
        Some(value) => value,
        None => panic!("constant must be non-zero"),
    }
}

fn default_target_branch() -> String {
    "main".to_owned()
}

fn default_claim_ttl_secs() -> NonZeroU64 {
    DEFAULT_CLAIM_TTL_SECS
}

fn default_sync_interval_secs() -> NonZeroU64 {
    DEFAULT_SYNC_INTERVAL_SECS
}

fn default_max_files_deleted_per_command() -> u32 {
    50
}

fn enabled() -> bool {
    true
}

/// The `[features]` table of `config.toml`: which parts of ACS run in this repository.
///
/// Every part except `sync` is on unless it is switched off here. The file is committed, so the
/// switches are the same for everyone who works in the repository, and it is sealed, so only a
/// person can change them. ACS's protection of itself is not a feature and cannot be switched
/// off: agents can never write `.acs/`, git's hooks and config or the harness's hook settings,
/// run the `acs` commands that change state, or skip the git hooks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Features {
    /// One writer per file: an edit claims the file, and a file another session holds is
    /// denied. Off: nothing is claimed or blocked by a claim, and the rule against deleting
    /// files outside one's claims does not apply.
    #[serde(default = "enabled")]
    pub claims: bool,
    /// The rules of `guards.toml` and their built-in defaults: sealed paths (other than ACS's
    /// own), banned commands, protected branches, the delete limits and the questions about
    /// writes ACS cannot place.
    #[serde(default = "enabled")]
    pub guards: bool,
    /// The zones of `territory.toml`. Off: the whole repository is `shared`.
    #[serde(default = "enabled")]
    pub territory: bool,
    /// Lore: showing entries to agents and taking their proposals.
    #[serde(default = "enabled")]
    pub lore: bool,
    /// Session history: the copies of files kept for `acs undo`, the session records behind
    /// `acs log`, `acs blame` and `acs stats`, and the `ACS-Session` trailer on commits.
    #[serde(default = "enabled")]
    pub history: bool,
    /// The checks in the git hooks: a commit of files someone else holds or of sealed paths,
    /// and an agent's force push to a protected branch.
    #[serde(default = "enabled")]
    pub commit_checks: bool,
    /// Team sync: publishing claims and session records to the git remote and fetching other
    /// machines'. Off by default: nothing leaves the machine until the repository opts in.
    #[serde(default)]
    pub sync: bool,
}

impl Default for Features {
    fn default() -> Self {
        Self {
            claims: true,
            guards: true,
            territory: true,
            lore: true,
            history: true,
            commit_checks: true,
            sync: false,
        }
    }
}

/// One switch of [`Features`], for code that names a feature to people.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Feature {
    /// [`Features::claims`]
    Claims,
    /// [`Features::guards`]
    Guards,
    /// [`Features::territory`]
    Territory,
    /// [`Features::lore`]
    Lore,
    /// [`Features::history`]
    History,
    /// [`Features::commit_checks`]
    CommitChecks,
    /// [`Features::sync`]
    Sync,
}

impl Feature {
    /// Every feature, in the order of the `[features]` table.
    pub const ALL: [Self; 7] = [
        Self::Claims,
        Self::Guards,
        Self::Territory,
        Self::Lore,
        Self::History,
        Self::CommitChecks,
        Self::Sync,
    ];

    /// The key in the `[features]` table.
    #[must_use]
    pub const fn key(self) -> &'static str {
        match self {
            Self::Claims => "claims",
            Self::Guards => "guards",
            Self::Territory => "territory",
            Self::Lore => "lore",
            Self::History => "history",
            Self::CommitChecks => "commit_checks",
            Self::Sync => "sync",
        }
    }

    /// What the feature does, in a few words.
    #[must_use]
    pub const fn summary(self) -> &'static str {
        match self {
            Self::Claims => "one writer per file: a file another session holds is blocked",
            Self::Guards => "sealed paths, banned commands, protected branches, delete limits",
            Self::Territory => "the zones of territory.toml (owned, open, sealed)",
            Self::Lore => "lore shown to agents, and their proposals",
            Self::History => {
                "copies for `acs undo`, session records (`acs log`, `acs blame`, `acs stats`), \
                 the ACS-Session commit trailer"
            }
            Self::CommitChecks => "git hooks that reject commits and pushes",
            Self::Sync => "claims and session records shared through the git remote",
        }
    }

    /// The feature with this key, if any.
    #[must_use]
    pub fn from_key(key: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|feature| feature.key() == key)
    }
}

impl fmt::Display for Feature {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.key())
    }
}

impl Features {
    /// Whether `feature` is on.
    #[must_use]
    pub const fn is_on(&self, feature: Feature) -> bool {
        match feature {
            Feature::Claims => self.claims,
            Feature::Guards => self.guards,
            Feature::Territory => self.territory,
            Feature::Lore => self.lore,
            Feature::History => self.history,
            Feature::CommitChecks => self.commit_checks,
            Feature::Sync => self.sync,
        }
    }

    /// The features that are off, in table order.
    #[must_use]
    pub fn off(&self) -> Vec<Feature> {
        Feature::ALL
            .into_iter()
            .filter(|feature| !self.is_on(*feature))
            .collect()
    }
}

/// Settings for the session ledger.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LedgerConfig {
    /// Whether user prompts are stored in the ledger. Defaults to `false`: prompts are never
    /// stored unless this is enabled.
    #[serde(default)]
    pub store_prompts: bool,
}

/// A whole percentage from 1 to 100.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "u8", into = "u8")]
pub struct Percent(u8);

impl Percent {
    /// The percentage, from 1 to 100.
    #[must_use]
    pub const fn get(self) -> u8 {
        self.0
    }
}

impl TryFrom<u8> for Percent {
    type Error = ParseError;

    fn try_from(value: u8) -> Result<Self, ParseError> {
        if (1..=100).contains(&value) {
            Ok(Self(value))
        } else {
            Err(ParseError::new(format!(
                "invalid percentage {value}: expected a whole number from 1 to 100"
            )))
        }
    }
}

impl From<Percent> for u8 {
    fn from(value: Percent) -> u8 {
        value.0
    }
}

impl JsonSchema for Percent {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "Percent".into()
    }

    fn json_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
        schemars::json_schema!({
            "description": "A whole percentage from 1 to 100.",
            "type": "integer",
            "minimum": 1,
            "maximum": 100
        })
    }
}

fn default_lore_max_inject_chars() -> NonZeroU32 {
    DEFAULT_LORE_MAX_INJECT_CHARS
}

fn default_lore_stale_percent() -> Percent {
    DEFAULT_LORE_STALE_PERCENT
}

/// Settings for lore (`.acs/policy/lore/`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LoreConfig {
    /// The most characters of lore one tool call may add to an agent's context. Entries that do
    /// not fit wait for the agent's next call; a single entry longer than this is cut, with a
    /// pointer to its file. Defaults to 4000.
    #[serde(default = "default_lore_max_inject_chars")]
    pub max_inject_chars: NonZeroU32,
    /// An entry is stale, and is served with a request to verify it, once at least this
    /// percentage of the lines in its scope changed since the revision it was written against.
    /// Defaults to 30.
    #[serde(default = "default_lore_stale_percent")]
    pub stale_percent: Percent,
}

impl Default for LoreConfig {
    fn default() -> Self {
        Self {
            max_inject_chars: DEFAULT_LORE_MAX_INJECT_CHARS,
            stale_percent: DEFAULT_LORE_STALE_PERCENT,
        }
    }
}

/// `.acs/policy/config.toml`: repository-wide settings. Only `spec` is required.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// The spec version the policy is written against. Always `acs/0.1`.
    pub spec: SpecVersion,
    /// The branch work is integrated into when a seat's own target is not known.
    #[serde(default = "default_target_branch")]
    pub default_target_branch: String,
    /// Seconds a claim stays live without a heartbeat before it expires.
    #[serde(default = "default_claim_ttl_secs")]
    pub claim_ttl_secs: NonZeroU64,
    /// Seconds between background syncs of claims and ledger with the git remote.
    #[serde(default = "default_sync_interval_secs")]
    pub sync_interval_secs: NonZeroU64,
    /// Ledger settings.
    #[serde(default)]
    pub ledger: LedgerConfig,
    /// Lore settings.
    #[serde(default)]
    pub lore: LoreConfig,
    /// Which parts of ACS run in this repository.
    #[serde(default)]
    pub features: Features,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            spec: SpecVersion,
            default_target_branch: default_target_branch(),
            claim_ttl_secs: DEFAULT_CLAIM_TTL_SECS,
            sync_interval_secs: DEFAULT_SYNC_INTERVAL_SECS,
            ledger: LedgerConfig::default(),
            lore: LoreConfig::default(),
            features: Features::default(),
        }
    }
}

/// The `[destructive]` table of `guards.toml`: limits on destructive shell commands.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Destructive {
    /// How many files a command may delete outside the seat's claims. `0` denies any such delete.
    #[serde(default)]
    pub max_files_deleted_outside_claims: u32,
    /// How many files one command may delete before the human is asked.
    #[serde(default = "default_max_files_deleted_per_command")]
    pub max_files_deleted_per_command: u32,
    /// Command patterns that are always denied, for example `git push --force*`. `*` matches any
    /// run of characters.
    #[serde(default)]
    pub ban: Vec<String>,
}

impl Default for Destructive {
    fn default() -> Self {
        Self {
            max_files_deleted_outside_claims: 0,
            max_files_deleted_per_command: default_max_files_deleted_per_command(),
            ban: Vec::new(),
        }
    }
}

/// `.acs/policy/guards.toml`: sealed paths, protected branches and destructive-command limits.
/// Every field defaults to empty; the built-in default guards are applied by the policy loader.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Guards {
    /// Optional spec version. If present it must be `acs/0.1`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spec: Option<SpecVersion>,
    /// Globs of paths agents may never write. See [`DEFAULT_SEALED`].
    #[serde(default)]
    pub sealed: Vec<String>,
    /// Branch name globs on which history rewrites and force pushes are denied.
    #[serde(default)]
    pub protected_branches: Vec<String>,
    /// Ask before an agent writes or deletes outside the repository (`path_escapes_repo`).
    /// Off by default: ACS guards the repository. Targets that cannot be resolved, and targets
    /// outside that link back into the repository, are asked about either way; protected
    /// locations such as `ACS_HOME` are always denied.
    #[serde(default)]
    pub protect_outside_repo: bool,
    /// Limits on destructive shell commands.
    #[serde(default)]
    pub destructive: Destructive,
}

/// How strictly a zone of the repository is controlled.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ZoneKind {
    /// Only the owners' agents may change it; other agents are denied (`owned_territory`)
    /// unless a human override covers the file.
    Owned,
    /// Any agent may change it after taking a claim.
    Shared,
    /// Any agent may change it without a claim.
    Open,
    /// No agent may write it.
    Sealed,
}

/// The owner of a zone or lore entry: a pilot (git email) or a team.
///
/// On the wire it is a single string: an email address, or `team:<name>` where `<name>` is a key
/// of the `[teams]` table in `territory.toml`.
#[derive(
    Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
)]
#[serde(try_from = "String", into = "String")]
#[schemars(extend("pattern" = r"^(?:team:[A-Za-z0-9_.-]+|[^@\s:]+@[^@\s:]+)$"))]
pub enum Owner {
    /// A single pilot, by git email address.
    Pilot(String),
    /// A team, by name.
    Team(String),
}

impl TryFrom<String> for Owner {
    type Error = ParseError;

    fn try_from(value: String) -> Result<Self, ParseError> {
        let invalid = || {
            ParseError::new(format!(
                "invalid owner {value:?}: expected an email address (name@domain) or \"team:<name>\""
            ))
        };
        if let Some(name) = value.strip_prefix("team:") {
            let ok = !name.is_empty()
                && name
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'-'));
            return if ok {
                Ok(Self::Team(name.to_owned()))
            } else {
                Err(invalid())
            };
        }
        match value.split_once('@') {
            Some((local, domain))
                if ![local, domain].iter().any(|part| {
                    part.is_empty()
                        || part.contains(|c: char| c == '@' || c == ':' || c.is_whitespace())
                }) =>
            {
                Ok(Self::Pilot(value))
            }
            _ => Err(invalid()),
        }
    }
}

impl From<Owner> for String {
    fn from(owner: Owner) -> String {
        match owner {
            Owner::Pilot(email) => email,
            Owner::Team(name) => format!("team:{name}"),
        }
    }
}

impl fmt::Display for Owner {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Pilot(email) => f.write_str(email),
            Self::Team(name) => write!(f, "team:{name}"),
        }
    }
}

/// One `[[zone]]` entry of `territory.toml`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Zone {
    /// Globs of the paths the zone covers. When zones overlap, the most specific match wins.
    pub paths: Vec<String>,
    /// How strictly the zone is controlled.
    pub kind: ZoneKind,
    /// Who owns the zone. Meaningful for `owned` zones; may be listed on any zone for review
    /// routing.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub owners: Vec<Owner>,
}

/// `.acs/policy/territory.toml`: who may change what. Every field defaults to empty, which makes
/// the whole repository `shared`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Territory {
    /// Optional spec version. If present it must be `acs/0.1`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spec: Option<SpecVersion>,
    /// The zones, written as `[[zone]]` tables.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub zone: Vec<Zone>,
    /// Team name to member git emails, referenced from `owners` as `team:<name>`.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub teams: BTreeMap<String, Vec<String>>,
}

/// The TOML front matter of a lore entry (a Markdown file under `.acs/policy/lore/`).
///
/// A lore file starts with a line `+++`, the front matter, a line `+++`, then the Markdown body.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LoreFrontMatter {
    /// Stable identifier of the entry.
    pub id: LoreId,
    /// Globs of the paths the entry applies to. The entry is injected the first time a seat
    /// enters one of them in a session.
    pub scope: Vec<String>,
    /// Who maintains the entry and reviews proposed changes.
    pub owner: Owner,
    /// The git revision the entry was written against; staleness is measured from it.
    pub written_against: String,
}

/// Splits a lore Markdown file into its TOML front matter and its body.
///
/// The text must start with a line consisting of `+++` (after an optional byte-order mark); the
/// front matter runs to the next line consisting of `+++`, and the body is everything after that
/// line, verbatim. Trailing whitespace on fence lines is ignored. The front matter is not parsed:
/// pass it to a TOML parser and deserialize it as [`LoreFrontMatter`].
///
/// # Errors
///
/// Returns an error if the opening or the closing fence is missing.
pub fn split_front_matter(text: &str) -> Result<(&str, &str), ParseError> {
    const FENCE: &str = "+++";
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut lines = text.split_inclusive('\n');
    let opening = lines
        .next()
        .filter(|line| line.trim_end() == FENCE)
        .ok_or_else(|| {
            ParseError::new("lore file must start with a front matter fence line \"+++\"")
        })?;
    let mut end = opening.len();
    for line in lines {
        if line.trim_end() == FENCE {
            return Ok((&text[opening.len()..end], &text[end + line.len()..]));
        }
        end += line.len();
    }
    Err(ParseError::new(
        "lore front matter is not closed by a second \"+++\" line",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_needs_only_spec_and_applies_defaults() {
        let config: Config = toml::from_str("spec = \"acs/0.1\"").unwrap();
        assert_eq!(config, Config::default());
        assert_eq!(config.default_target_branch, "main");
        assert_eq!(config.claim_ttl_secs.get(), 900);
        assert_eq!(config.sync_interval_secs.get(), 30);
        assert!(!config.ledger.store_prompts);
    }

    #[test]
    fn features_default_to_on_except_sync_and_reject_typos() {
        let config: Config = toml::from_str("spec = \"acs/0.1\"").unwrap();
        assert_eq!(config.features.off(), [Feature::Sync]);
        let config: Config = toml::from_str(
            "spec = \"acs/0.1\"\n[features]\nclaims = false\nsync = true\ncommit_checks = false",
        )
        .unwrap();
        assert_eq!(
            config.features.off(),
            [Feature::Claims, Feature::CommitChecks]
        );
        assert!(config.features.territory && config.features.sync);
        let err = toml::from_str::<Config>("spec = \"acs/0.1\"\n[features]\nclaim = false")
            .unwrap_err()
            .to_string();
        assert!(err.contains("unknown field `claim`"), "{err}");
        for feature in Feature::ALL {
            assert_eq!(Feature::from_key(feature.key()), Some(feature));
        }
    }

    #[test]
    fn self_protection_seals_are_default_seals() {
        for glob in SELF_PROTECTION_SEALED {
            assert!(DEFAULT_SEALED.contains(glob), "{glob}");
        }
    }

    #[test]
    fn config_rejects_bad_input() {
        let cases = [
            ("", "missing field `spec`"),
            ("spec = \"acs/0.2\"", "unsupported spec version"),
            (
                "spec = \"acs/0.1\"\nclaim_ttl = 5",
                "unknown field `claim_ttl`",
            ),
            ("spec = \"acs/0.1\"\nclaim_ttl_secs = 0", "nonzero"),
            (
                "spec = \"acs/0.1\"\n[ledger]\nstore_promts = true",
                "unknown field `store_promts`",
            ),
        ];
        for (input, expected) in cases {
            let err = toml::from_str::<Config>(input).unwrap_err().to_string();
            assert!(err.contains(expected), "{input:?} -> {err}");
        }
    }

    #[test]
    fn guards_and_territory_default_to_empty_and_reject_unknown_fields() {
        assert_eq!(toml::from_str::<Guards>("").unwrap(), Guards::default());
        assert_eq!(
            toml::from_str::<Territory>("").unwrap(),
            Territory::default()
        );
        assert_eq!(
            Guards::default().destructive.max_files_deleted_per_command,
            50
        );
        for (input, expected) in [
            ("sealed_paths = []", "unknown field `sealed_paths`"),
            ("[destructive]\nbans = []", "unknown field `bans`"),
        ] {
            let err = toml::from_str::<Guards>(input).unwrap_err().to_string();
            assert!(err.contains(expected), "{err}");
        }
        let err = toml::from_str::<Territory>(
            "[[zone]]\npaths = [\"a\"]\nkind = \"owned\"\nowner = \"x@y\"",
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("unknown field `owner`"), "{err}");
        assert!(
            toml::from_str::<Territory>("[[zone]]\npaths = [\"a\"]\nkind = \"private\"").is_err()
        );
    }

    #[test]
    fn owner_parsing() {
        let cases: &[(&str, Option<Owner>)] = &[
            (
                "sara@example.com",
                Some(Owner::Pilot("sara@example.com".into())),
            ),
            ("team:platform", Some(Owner::Team("platform".into()))),
            (
                "team:core-infra_2.0",
                Some(Owner::Team("core-infra_2.0".into())),
            ),
            ("team:", None),
            ("team:a b", None),
            ("sara", None),
            ("@example.com", None),
            ("sara@", None),
            ("a@b@c", None),
            ("sara @example.com", None),
            ("", None),
        ];
        for (input, expected) in cases {
            let parsed = Owner::try_from((*input).to_owned()).ok();
            assert_eq!(&parsed, expected, "{input:?}");
            if let Some(owner) = parsed {
                assert_eq!(owner.to_string(), *input);
                assert_eq!(String::from(owner), *input);
            }
        }
    }

    #[test]
    fn split_front_matter_table() {
        let cases: &[(&str, Option<(&str, &str)>)] = &[
            ("+++\na = 1\n+++\nbody\n", Some(("a = 1\n", "body\n"))),
            ("+++\r\na = 1\r\n+++\r\nbody", Some(("a = 1\r\n", "body"))),
            ("\u{feff}+++\na = 1\n+++\n", Some(("a = 1\n", ""))),
            ("+++\n+++\n\nx\n+++\n", Some(("", "\nx\n+++\n"))),
            ("+++ \na\n+++  \nb", Some(("a\n", "b"))),
            ("no fence\n+++\n+++\n", None),
            ("\n+++\na\n+++\n", None),
            ("+++\na = 1\n", None),
            ("+++", None),
            ("", None),
        ];
        for &(input, expected) in cases {
            assert_eq!(split_front_matter(input).ok(), expected, "{input:?}");
        }
    }
}
