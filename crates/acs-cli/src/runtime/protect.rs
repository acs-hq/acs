//! What the engine needs for territory and self-protection that only the runtime knows: the
//! overrides in force for a seat, and the locations outside the repository agents must never
//! write (decision 0026).

use std::path::{Path, PathBuf};

use acs_core::gate::{OutsideSeal, OverrideView};
use acs_core::{OverrideId, RepoPath, SeatId, Timestamp};
use acs_policy::glob::Glob;
use acs_store::StateDb;

use super::RepoContext;

/// The overrides in force for one seat, with their globs compiled in the policy's dialect.
#[derive(Debug, Default)]
pub struct StoreOverrides(Vec<(Glob, OverrideId)>);

impl StoreOverrides {
    /// Loads the overrides `seat` may use at `now`. A stored glob that no longer compiles is
    /// skipped (it then covers nothing).
    ///
    /// # Errors
    ///
    /// Any [`acs_store::StoreError`].
    pub fn load(
        ctx: &RepoContext,
        db: &StateDb,
        seat: &SeatId,
        now: Timestamp,
    ) -> Result<Self, acs_store::StoreError> {
        let case_insensitive = ctx.resolver.case_insensitive();
        Ok(Self(
            db.overrides_for_seat(seat, now)?
                .into_iter()
                .filter_map(|o| {
                    Glob::new(&o.pattern, case_insensitive)
                        .ok()
                        .map(|glob| (glob, o.override_id))
                })
                .collect(),
        ))
    }
}

impl OverrideView for StoreOverrides {
    fn covering(&self, path: &RepoPath) -> Option<&OverrideId> {
        self.0
            .iter()
            .find(|(glob, _)| glob.is_match(path.as_str()))
            .map(|(_, id)| id)
    }
}

/// The locations outside the work tree that agents may never write or delete: `ACS_HOME`, git
/// directories outside the work tree (the common directory of a linked worktree), and the
/// user-level configuration of Claude Code and Antigravity that can turn ACS off.
#[must_use]
pub fn outside_seals(ctx: &RepoContext) -> Vec<OutsideSeal> {
    let home = std::env::home_dir().filter(|p| !p.as_os_str().is_empty());
    let mut seals = Vec::new();
    let mut add = |path: PathBuf, label: &str, dir: bool| {
        let home_relative = home.as_deref().and_then(|h| relative(&path, h, dir));
        seals.push(OutsideSeal {
            path: canonical_prefix(&path).to_string_lossy().into_owned(),
            label: label.to_owned(),
            home_relative,
        });
    };
    if let Some(acs_home) = &ctx.acs_home {
        add(
            acs_home.root().to_path_buf(),
            "ACS state (`ACS_HOME`)",
            true,
        );
    }
    let work_tree = canonical_prefix(&ctx.layout.work_tree);
    for dir in [&ctx.layout.common_dir, &ctx.layout.git_dir] {
        if !canonical_prefix(dir).starts_with(&work_tree) {
            add(dir.clone(), "the repository's git directory", true);
        }
    }
    let claude = std::env::var_os("CLAUDE_CONFIG_DIR")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| home.as_ref().map(|h| h.join(".claude")));
    if let Some(claude) = claude {
        add(
            claude.join("settings.json"),
            "Claude Code user settings",
            false,
        );
        add(
            claude.join("settings.local.json"),
            "Claude Code user settings",
            false,
        );
        add(claude.join("plugins"), "Claude Code plugins", true);
    }
    if let Some(home) = &home {
        let gemini = home.join(".gemini");
        let config = gemini.join("config");
        add(config.join("hooks.json"), "Antigravity user hooks", false);
        add(config.join("plugins"), "Antigravity plugins", true);
        add(
            config.join("config.json"),
            "Antigravity plugin settings",
            false,
        );
        add(
            gemini.join("antigravity-cli").join("settings.json"),
            "Antigravity CLI settings",
            false,
        );
    }
    seals
}

/// `path` relative to `home`, `/`-separated, with a trailing `/` for a directory.
fn relative(path: &Path, home: &Path, dir: bool) -> Option<String> {
    let rest = path.strip_prefix(home).ok()?;
    let mut text = rest
        .components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join("/");
    if text.is_empty() {
        return None;
    }
    if dir {
        text.push('/');
    }
    Some(text)
}

/// `path` with its longest existing ancestor canonicalized (links resolved), so it compares
/// equal to the resolver's canonical form of a target.
fn canonical_prefix(path: &Path) -> PathBuf {
    let mut rest = Vec::new();
    let mut current = path;
    loop {
        if let Ok(canonical) = std::fs::canonicalize(current) {
            let mut out = simplify(canonical);
            for part in rest.iter().rev() {
                out.push(part);
            }
            return out;
        }
        match (current.parent(), current.file_name()) {
            (Some(parent), Some(name)) => {
                rest.push(name.to_owned());
                current = parent;
            }
            _ => return path.to_path_buf(),
        }
    }
}

/// Drops the Windows verbatim prefix (`\\?\C:\` becomes `C:\`) when it is a plain drive path.
fn simplify(path: PathBuf) -> PathBuf {
    let text = path.to_string_lossy();
    match text.strip_prefix(r"\\?\") {
        Some(rest) if rest.as_bytes().get(1) == Some(&b':') => PathBuf::from(rest),
        _ => path,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relative_paths_mark_directories() {
        let home = Path::new("/home/me");
        assert_eq!(
            relative(Path::new("/home/me/.claude/settings.json"), home, false).as_deref(),
            Some(".claude/settings.json")
        );
        assert_eq!(
            relative(Path::new("/home/me/.acs"), home, true).as_deref(),
            Some(".acs/")
        );
        assert_eq!(relative(Path::new("/opt/acs"), home, true), None);
        assert_eq!(relative(home, home, true), None);
    }

    #[test]
    fn missing_tails_are_kept_after_the_canonical_ancestor() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("a").join("b.json");
        let got = canonical_prefix(&missing);
        assert!(got.ends_with(Path::new("a").join("b.json")), "{got:?}");
        assert!(!got.to_string_lossy().starts_with(r"\\?\"), "{got:?}");
    }
}
