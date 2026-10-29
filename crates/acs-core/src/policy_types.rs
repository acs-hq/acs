//! Types of the committed policy files under `.acs/policy/`: `config.toml`, `guards.toml`,
//! `territory.toml`, and the front matter of lore entries.
//!
//! These types reject unknown fields so that a typo in a policy file is an error, not a silently
//! ignored setting.

use std::collections::BTreeMap;
use std::fmt;
use std::num::NonZeroU64;

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
    ".claude/settings*.json",
    ".agents/**/hooks*.json",
];

const DEFAULT_CLAIM_TTL_SECS: NonZeroU64 = non_zero(900);
const DEFAULT_SYNC_INTERVAL_SECS: NonZeroU64 = non_zero(30);

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

/// Settings for the session ledger.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LedgerConfig {
    /// Whether user prompts are stored in the ledger. Defaults to `false`: prompts are never
    /// stored unless this is enabled.
    #[serde(default)]
    pub store_prompts: bool,
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
}

impl Default for Config {
    fn default() -> Self {
        Self {
            spec: SpecVersion,
            default_target_branch: default_target_branch(),
            claim_ttl_secs: DEFAULT_CLAIM_TTL_SECS,
            sync_interval_secs: DEFAULT_SYNC_INTERVAL_SECS,
            ledger: LedgerConfig::default(),
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
    /// Limits on destructive shell commands.
    #[serde(default)]
    pub destructive: Destructive,
}

/// How strictly a zone of the repository is controlled.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ZoneKind {
    /// Only the owners' agents may change it; other agents are denied. Implementations that do
    /// not yet enforce ownership MUST treat `owned` zones as `shared`.
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
