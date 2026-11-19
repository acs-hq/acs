//! The ACS block in a Codex `config.toml` (decision 0047): the hooks and the MCP server that
//! `acs plugin install codex` adds and `acs uninstall` takes out again.
//!
//! Codex reads hooks and MCP servers from `config.toml` in each layer: the user's
//! (`$CODEX_HOME/config.toml`, default `~/.codex/config.toml`) and a trusted project's
//! (`.codex/config.toml`). The block is plain text between two marker lines, appended at the end
//! of the file, so the rest of the file is never rewritten and taking the block out restores it
//! byte for byte. Its tables are `[mcp_servers.acs]` and `[[hooks.<Event>]]` entries, which add
//! to any hooks the file already has.

use std::path::{Path, PathBuf};

/// The block, built in from `integrations/codex/config.toml`.
pub const BLOCK: &str = include_str!("../../../../integrations/codex/config.toml");

/// The block's first line starts with this.
pub const START: &str = "# >>> acs";
/// The block's last line.
pub const END: &str = "# <<< acs";

/// The project's Codex configuration file, relative to the work tree root.
pub const PROJECT_FILE: &str = ".codex/config.toml";

/// The project's Codex configuration file.
#[must_use]
pub fn project_file(work_tree: &Path) -> PathBuf {
    work_tree.join(PROJECT_FILE)
}

/// Codex's home folder: `$CODEX_HOME`, else `~/.codex`.
#[must_use]
pub fn codex_home() -> Option<PathBuf> {
    match std::env::var_os("CODEX_HOME").filter(|v| !v.is_empty()) {
        Some(dir) => Some(PathBuf::from(dir)),
        None => std::env::home_dir()
            .filter(|p| !p.as_os_str().is_empty())
            .map(|h| h.join(".codex")),
    }
}

/// The user's Codex configuration file.
#[must_use]
pub fn user_file() -> Option<PathBuf> {
    codex_home().map(|dir| dir.join("config.toml"))
}

/// Where the block is in `text`, as byte offsets: where the blank line that [`add_block`] put
/// in front of it starts (the block's own start when there is none), where the start marker
/// line starts, and where the end marker line ends (its line ending included).
fn span(text: &str) -> Option<(usize, usize, usize)> {
    let mut offset = 0;
    let mut start = None;
    for line in text.split_inclusive('\n') {
        let bare = line.trim_end_matches(['\r', '\n']);
        if start.is_none() && bare.starts_with(START) {
            start = Some(offset);
        } else if let Some(begin) = start
            && bare == END
        {
            let before = &text[..begin];
            let lead = if before.ends_with("\r\n\r\n") {
                begin - 2
            } else if before.ends_with("\n\n") {
                begin - 1
            } else {
                begin
            };
            return Some((lead, begin, offset + line.len()));
        }
        offset += line.len();
    }
    None
}

/// Whether `text` holds the block (any version of it).
#[must_use]
pub fn has_block(text: &str) -> bool {
    span(text).is_some()
}

/// Whether the file holds the block.
#[must_use]
pub fn file_has_block(file: &Path) -> bool {
    std::fs::read_to_string(file).is_ok_and(|text| has_block(&text))
}

/// `text` with the current block in it: an older block is replaced where it stands, otherwise
/// the block is appended after a blank line. `None` when `text` already holds this block.
///
/// # Errors
///
/// A message when the file is not valid TOML, or would not be with the block (it already
/// defines `mcp_servers.acs`, or defines hooks in a form the block's tables cannot add to).
pub fn add_block(text: &str) -> Result<Option<String>, String> {
    if text.contains(BLOCK) {
        return Ok(None);
    }
    toml::from_str::<toml::Table>(text).map_err(|err| format!("not valid TOML: {err}"))?;
    let new = match span(text) {
        Some((_, begin, end)) => format!("{}{BLOCK}{}", &text[..begin], &text[end..]),
        None if text.is_empty() => BLOCK.to_owned(),
        None if text.ends_with('\n') => format!("{text}\n{BLOCK}"),
        None => format!("{text}\n\n{BLOCK}"),
    };
    toml::from_str::<toml::Table>(&new).map_err(|err| {
        format!(
            "the ACS block does not fit in it ({err}). It may already define `mcp_servers.acs` \
             or set hooks in another form; add the block from integrations/codex/config.toml \
             by hand."
        )
    })?;
    Ok(Some(new))
}

/// `text` without the block, or `None` when it holds none.
#[must_use]
pub fn remove_block(text: &str) -> Option<String> {
    let (lead, _, end) = span(text)?;
    Some(format!("{}{}", &text[..lead], &text[end..]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_block_is_valid_toml_with_the_server_and_four_hooks() {
        let table: toml::Table = toml::from_str(BLOCK).expect("toml");
        assert_eq!(table["mcp_servers"]["acs"]["command"].as_str(), Some("acs"));
        let hooks = table["hooks"].as_table().expect("hooks");
        let events: Vec<&str> = hooks.keys().map(String::as_str).collect();
        assert_eq!(
            events,
            ["PostToolUse", "PreToolUse", "SessionEnd", "SessionStart"]
        );
        for (_, groups) in hooks {
            let handler = &groups[0]["hooks"][0];
            assert_eq!(handler["command"].as_str(), Some("acs hook codex"));
        }
        assert!(BLOCK.starts_with(START) && BLOCK.ends_with(&format!("{END}\n")));
    }

    #[test]
    fn adding_then_removing_restores_the_file_byte_for_byte() {
        for original in [
            "",
            "model = \"o3\"\n",
            "model = \"o3\"\n\n[[hooks.PreToolUse]]\nmatcher = \"^Bash$\"\n[[hooks.PreToolUse.hooks]]\ntype = \"command\"\ncommand = \"lint\"\n",
            "model = \"o3\"\r\n",
        ] {
            let added = add_block(original).expect("add").expect("changed");
            assert!(has_block(&added));
            toml::from_str::<toml::Table>(&added).expect("still toml");
            assert_eq!(add_block(&added).expect("again"), None, "idempotent");
            assert_eq!(remove_block(&added).as_deref(), Some(original));
        }
        assert_eq!(remove_block("model = \"o3\"\n"), None);
    }

    #[test]
    fn an_older_block_is_replaced_in_place() {
        let old = format!("a = 1\n\n{START}\n[mcp_servers.acs]\ncommand = \"old\"\n{END}\nb = 2\n");
        let new = add_block(&old).expect("add").expect("changed");
        assert!(new.starts_with("a = 1\n\n# >>> acs"));
        assert!(new.ends_with(&format!("{END}\nb = 2\n")));
        assert!(!new.contains("\"old\""));
    }

    #[test]
    fn a_file_that_cannot_take_the_block_is_refused() {
        assert!(add_block("[mcp_servers.acs]\ncommand = \"x\"\n").is_err());
        assert!(add_block("not = [toml\n").is_err());
    }
}
