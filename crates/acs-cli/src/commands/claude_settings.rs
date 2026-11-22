//! Putting the ACS plugin into a Claude Code settings file, and taking it out, without disturbing
//! the rest of it.
//!
//! The plugin is two entries: `enabledPlugins."acs@acs-plugins"` and
//! `extraKnownMarketplaces."acs-plugins"`. `acs plugin install claude-code` has `claude plugin
//! ...` write them, or writes them itself with [`set_entries`] when `claude` is not on `PATH`
//! (issue 83); `acs uninstall` removes exactly those with [`remove_entries`]. Every other entry
//! keeps its text, its order and its formatting: the file is read as a list of `(key, raw JSON
//! text)` pairs, so values that are not touched are copied byte for byte, and the file is written
//! back in the layout it had (Claude Code writes two-space indented JSON). An object left empty is
//! removed; a file left empty is deleted.

use std::fmt;

use serde::de::{Deserialize, Deserializer, MapAccess, Visitor};
use serde_json::Value;
use serde_json::value::RawValue;

/// A JSON object as its entries in file order, each value as its original text.
struct Entries(Vec<(String, Box<RawValue>)>);

impl<'de> Deserialize<'de> for Entries {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct EntriesVisitor;
        impl<'de> Visitor<'de> for EntriesVisitor {
            type Value = Entries;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a JSON object")
            }
            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Entries, A::Error> {
                let mut entries = Vec::new();
                while let Some(entry) = map.next_entry::<String, Box<RawValue>>()? {
                    entries.push(entry);
                }
                Ok(Entries(entries))
            }
        }
        deserializer.deserialize_map(EntriesVisitor)
    }
}

/// The outcome of editing one settings file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Edit {
    /// Nothing of ACS's is in it.
    Unchanged,
    /// The entries named were removed; the new text follows.
    Changed {
        /// `enabledPlugins."acs@acs-plugins"` and the like, for the report.
        removed: Vec<String>,
        /// The new file text.
        text: String,
    },
    /// Only ACS's entries were in it: the file can be deleted.
    Empty {
        /// As for [`Edit::Changed`].
        removed: Vec<String>,
    },
}

/// The indentation unit of `text` (the whitespace before its first key), or `None` when it is
/// written on one line.
fn indent_unit(text: &str) -> Option<String> {
    let after_brace = &text[text.find('{')? + 1..];
    let newline = after_brace.find('\n')?;
    // Only whitespace may come between the brace and the first newline.
    if !after_brace[..newline].trim().is_empty() {
        return None;
    }
    let line = &after_brace[newline + 1..];
    let unit: String = line
        .chars()
        .take_while(|c| *c == ' ' || *c == '\t')
        .collect();
    (!unit.is_empty()).then_some(unit)
}

/// Writes an object at nesting `depth` (0 for the file itself) in the file's layout.
fn render(entries: &[(String, String)], unit: Option<&str>, depth: usize) -> String {
    let key = |k: &str| serde_json::to_string(k).unwrap_or_else(|_| format!("\"{k}\""));
    match unit {
        None => {
            let body: Vec<String> = entries
                .iter()
                .map(|(k, v)| format!("{}:{v}", key(k)))
                .collect();
            format!("{{{}}}", body.join(","))
        }
        Some(unit) => {
            let inner = unit.repeat(depth + 1);
            let outer = unit.repeat(depth);
            let body: Vec<String> = entries
                .iter()
                .map(|(k, v)| format!("{inner}{}: {v}", key(k)))
                .collect();
            format!("{{\n{}\n{outer}}}", body.join(",\n"))
        }
    }
}

/// Removes `remove` (pairs of top-level key and key inside it) from the settings text.
///
/// # Errors
///
/// A message when the text is not a JSON object.
pub fn remove_entries(text: &str, remove: &[(&str, &str)]) -> Result<Edit, String> {
    let top: Entries = serde_json::from_str(text).map_err(|err| err.to_string())?;
    let unit = indent_unit(text);
    let mut removed = Vec::new();
    let mut out: Vec<(String, String)> = Vec::new();
    for (key, value) in top.0 {
        let names: Vec<&str> = remove
            .iter()
            .filter(|(outer, _)| *outer == key)
            .map(|(_, inner)| *inner)
            .collect();
        if names.is_empty() {
            out.push((key, value.get().to_owned()));
            continue;
        }
        let Ok(inner) = serde_json::from_str::<Entries>(value.get()) else {
            out.push((key, value.get().to_owned()));
            continue;
        };
        let before = inner.0.len();
        let kept: Vec<(String, String)> = inner
            .0
            .into_iter()
            .filter(|(k, _)| {
                let hit = names.contains(&k.as_str());
                if hit {
                    removed.push(format!("{key}.{k}"));
                }
                !hit
            })
            .map(|(k, v)| (k, v.get().to_owned()))
            .collect();
        if kept.len() == before {
            out.push((key, value.get().to_owned()));
        } else if !kept.is_empty() {
            out.push((key, render(&kept, unit.as_deref(), 1)));
        }
    }
    if removed.is_empty() {
        return Ok(Edit::Unchanged);
    }
    if out.is_empty() {
        return Ok(Edit::Empty { removed });
    }
    // Keep what surrounded the object (a final newline, usually).
    let start = text.find('{').unwrap_or(0);
    let end = text.rfind('}').map_or(text.len(), |i| i + 1);
    let text = format!(
        "{}{}{}",
        &text[..start],
        render(&out, unit.as_deref(), 0),
        &text[end..]
    );
    Ok(Edit::Changed { removed, text })
}

/// Writes a new value, given as JSON text, at nesting `depth` in the file's layout: objects are
/// laid out like the file with their keys in the order given, everything else as it was given.
fn render_value(value: &str, unit: Option<&str>, depth: usize) -> String {
    match serde_json::from_str::<Entries>(value) {
        Ok(entries) if !entries.0.is_empty() => {
            let entries: Vec<(String, String)> = entries
                .0
                .iter()
                .map(|(k, v)| (k.clone(), render_value(v.get(), unit, depth + 1)))
                .collect();
            render(&entries, unit, depth)
        }
        _ => value.trim().to_owned(),
    }
}

/// Sets `set` (triples of top-level key, key inside it and value as JSON text) in the settings
/// text. Empty text stands for a file that does not exist yet. An entry that already holds an
/// equal value is left alone; a different value is replaced. Returns `None` when nothing changes.
///
/// # Errors
///
/// A message when the text is not a JSON object, a value is not JSON, or a top-level key to add
/// into holds something other than an object.
pub fn set_entries(text: &str, set: &[(&str, &str, &str)]) -> Result<Option<String>, String> {
    let new_file = text.trim().is_empty();
    let top: Entries = if new_file {
        Entries(Vec::new())
    } else {
        serde_json::from_str(text).map_err(|err| err.to_string())?
    };
    // A file with nothing in it yet gets Claude Code's own layout.
    let unit = indent_unit(text).or_else(|| top.0.is_empty().then(|| "  ".to_owned()));
    let unit = unit.as_deref();
    let mut out: Vec<(String, String)> = top
        .0
        .into_iter()
        .map(|(k, v)| (k, v.get().to_owned()))
        .collect();
    let mut changed = false;
    for (outer, inner, value_text) in set {
        let value: Value = serde_json::from_str(value_text)
            .map_err(|err| format!("the value for `{outer}.{inner}`: {err}"))?;
        let rendered = render_value(value_text, unit, 2);
        let Some(slot) = out.iter_mut().find(|(k, _)| k == outer) else {
            out.push((
                (*outer).to_owned(),
                render(&[((*inner).to_owned(), rendered)], unit, 1),
            ));
            changed = true;
            continue;
        };
        let entries: Entries = serde_json::from_str(&slot.1)
            .map_err(|_| format!("`{outer}` holds something other than an object"))?;
        let mut entries: Vec<(String, String)> = entries
            .0
            .into_iter()
            .map(|(k, v)| (k, v.get().to_owned()))
            .collect();
        match entries.iter_mut().find(|(k, _)| k == inner) {
            Some((_, old)) if serde_json::from_str::<Value>(old).ok().as_ref() == Some(&value) => {
                continue;
            }
            Some((_, old)) => *old = rendered,
            None => entries.push(((*inner).to_owned(), rendered)),
        }
        slot.1 = render(&entries, unit, 1);
        changed = true;
    }
    if !changed {
        return Ok(None);
    }
    if new_file {
        return Ok(Some(format!("{}\n", render(&out, unit, 0))));
    }
    let start = text.find('{').unwrap_or(0);
    let end = text.rfind('}').map_or(text.len(), |i| i + 1);
    Ok(Some(format!(
        "{}{}{}",
        &text[..start],
        render(&out, unit, 0),
        &text[end..]
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    const ACS: &[(&str, &str)] = &[
        ("enabledPlugins", "acs@acs-plugins"),
        ("extraKnownMarketplaces", "acs-plugins"),
    ];

    #[test]
    fn only_acs_entries_are_removed_and_the_rest_is_untouched() {
        let before = "{\n  \"permissions\": {\n    \"allow\": [\n      \"Bash(ls)\"\n    ]\n  },\n  \"enabledPlugins\": {\n    \"other@x\": true\n  },\n  \"hooks\": {}\n}\n";
        let with_acs = "{\n  \"permissions\": {\n    \"allow\": [\n      \"Bash(ls)\"\n    ]\n  },\n  \"enabledPlugins\": {\n    \"other@x\": true,\n    \"acs@acs-plugins\": true\n  },\n  \"hooks\": {},\n  \"extraKnownMarketplaces\": {\n    \"acs-plugins\": {\n      \"source\": {\n        \"source\": \"github\",\n        \"repo\": \"acs-hq/acs\"\n      }\n    }\n  }\n}\n";
        let Edit::Changed { removed, text } = remove_entries(with_acs, ACS).unwrap() else {
            panic!("expected a change");
        };
        assert_eq!(text, before);
        assert_eq!(
            removed,
            [
                "enabledPlugins.acs@acs-plugins",
                "extraKnownMarketplaces.acs-plugins"
            ]
        );
    }

    #[test]
    fn a_file_with_only_acs_entries_is_empty() {
        let text = "{\n  \"enabledPlugins\": {\n    \"acs@acs-plugins\": true\n  }\n}";
        assert!(matches!(
            remove_entries(text, ACS).unwrap(),
            Edit::Empty { .. }
        ));
    }

    #[test]
    fn a_file_without_acs_is_unchanged_and_one_line_files_stay_one_line() {
        assert_eq!(remove_entries("{\"a\": 1}", ACS).unwrap(), Edit::Unchanged);
        let Edit::Changed { text, .. } = remove_entries(
            "{\"a\":1,\"enabledPlugins\":{\"acs@acs-plugins\":true,\"b@c\":false}}",
            ACS,
        )
        .unwrap() else {
            panic!("expected a change");
        };
        assert_eq!(text, "{\"a\":1,\"enabledPlugins\":{\"b@c\":false}}");
    }

    #[test]
    fn text_that_is_not_an_object_is_an_error() {
        assert!(remove_entries("[1]", ACS).is_err());
        assert!(remove_entries("not json", ACS).is_err());
    }

    fn acs_set() -> Vec<(&'static str, &'static str, &'static str)> {
        vec![
            (
                "extraKnownMarketplaces",
                "acs-plugins",
                r#"{"source": {"source": "github", "repo": "acs-hq/acs"}}"#,
            ),
            ("enabledPlugins", "acs@acs-plugins", "true"),
        ]
    }

    #[test]
    fn a_new_file_gets_claude_codes_layout() {
        let text = set_entries("", &acs_set()).unwrap().unwrap();
        assert_eq!(
            text,
            "{\n  \"extraKnownMarketplaces\": {\n    \"acs-plugins\": {\n      \"source\": {\n        \"source\": \"github\",\n        \"repo\": \"acs-hq/acs\"\n      }\n    }\n  },\n  \"enabledPlugins\": {\n    \"acs@acs-plugins\": true\n  }\n}\n"
        );
        // What it wrote is what uninstall takes out again.
        assert!(matches!(
            remove_entries(&text, ACS).unwrap(),
            Edit::Empty { .. }
        ));
        assert_eq!(set_entries("{}\n", &acs_set()).unwrap().unwrap(), text);
    }

    #[test]
    fn entries_join_the_file_and_the_rest_is_untouched() {
        let before = "{\n  \"permissions\": {\n    \"allow\": [\n      \"Bash(ls)\"\n    ]\n  },\n  \"enabledPlugins\": {\n    \"other@x\": true\n  }\n}\n";
        let text = set_entries(before, &acs_set()).unwrap().unwrap();
        assert_eq!(
            text,
            "{\n  \"permissions\": {\n    \"allow\": [\n      \"Bash(ls)\"\n    ]\n  },\n  \"enabledPlugins\": {\n    \"other@x\": true,\n    \"acs@acs-plugins\": true\n  },\n  \"extraKnownMarketplaces\": {\n    \"acs-plugins\": {\n      \"source\": {\n        \"source\": \"github\",\n        \"repo\": \"acs-hq/acs\"\n      }\n    }\n  }\n}\n"
        );
        let Edit::Changed { text: back, .. } = remove_entries(&text, ACS).unwrap() else {
            panic!("expected a change");
        };
        assert_eq!(back, before);
    }

    #[test]
    fn entries_already_there_change_nothing_and_a_disabled_plugin_is_enabled() {
        let done = set_entries("", &acs_set()).unwrap().unwrap();
        assert_eq!(set_entries(&done, &acs_set()).unwrap(), None);
        let disabled = done.replace("\"acs@acs-plugins\": true", "\"acs@acs-plugins\": false");
        assert_eq!(set_entries(&disabled, &acs_set()).unwrap().unwrap(), done);
    }

    #[test]
    fn one_line_files_stay_one_line_and_bad_shapes_are_errors() {
        let set = [("enabledPlugins", "acs@acs-plugins", "true")];
        assert_eq!(
            set_entries("{\"a\":1}", &set).unwrap().unwrap(),
            "{\"a\":1,\"enabledPlugins\":{\"acs@acs-plugins\":true}}"
        );
        assert!(set_entries("{\"enabledPlugins\": []}", &set).is_err());
        assert!(set_entries("not json", &set).is_err());
    }
}
