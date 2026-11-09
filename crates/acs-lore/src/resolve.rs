//! Which lore applies to a set of paths, and the text an agent is shown.

use acs_policy::glob::Specificity;

use crate::format::{LoreEntry, LoreSet};
use crate::stale::Staleness;

/// An entry that applies to at least one of the paths asked about.
#[derive(Debug, Clone)]
pub struct Match<'a> {
    /// The entry.
    pub entry: &'a LoreEntry,
    /// How specifically its scope covers the best-covered path.
    pub specificity: Specificity,
    /// The first path, in the order given, that the entry covers.
    pub path: String,
}

/// The entries of `set` whose scope covers any of `paths` (repository-relative,
/// `/`-separated), most specific first. Ties keep file path order.
#[must_use]
pub fn resolve<'a>(set: &'a LoreSet, paths: &[&str]) -> Vec<Match<'a>> {
    let mut found: Vec<Match<'a>> = set
        .entries()
        .iter()
        .filter_map(|entry| {
            let mut best: Option<(Specificity, &str)> = None;
            for path in paths {
                if let Some(spec) = entry.match_specificity(path)
                    && best.is_none_or(|(b, _)| spec > b)
                {
                    best = Some((spec, path));
                }
            }
            let first = paths.iter().find(|p| entry.covers(p))?;
            best.map(|(specificity, _)| Match {
                entry,
                specificity,
                path: (*first).to_owned(),
            })
        })
        .collect();
    // Stable: equal specificity keeps the set's (file path) order.
    found.sort_by_key(|m| std::cmp::Reverse(m.specificity));
    found
}

/// An entry about to be shown, with what is known about its staleness.
#[derive(Debug, Clone, Copy)]
pub struct Served<'a> {
    /// The entry.
    pub entry: &'a LoreEntry,
    /// `None` when staleness was not measured.
    pub staleness: Option<&'a Staleness>,
}

/// What [`render`] produced.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Rendered {
    /// One block of text per entry shown, in order.
    pub blocks: Vec<String>,
    /// The entries shown (in full or cut), in order.
    pub shown: Vec<usize>,
    /// The entries cut to fit.
    pub cut: Vec<usize>,
    /// The entries left for a later call because they did not fit.
    pub deferred: Vec<usize>,
}

/// Renders `items` for an agent within `cap` characters in total.
///
/// Entries are taken in order while they fit; one that does not fit is left for a later call
/// (indices in [`Rendered::deferred`]) and smaller ones after it may still be shown. When not
/// even the first entry fits, its body is cut and the block points to the file, so the most
/// specific lore always gets through. Indices refer to `items`.
#[must_use]
pub fn render(items: &[Served<'_>], cap: usize) -> Rendered {
    let mut out = Rendered::default();
    let mut used = 0usize;
    for (index, item) in items.iter().enumerate() {
        let header = header(item);
        let full = format!("{header}\n{}", item.entry.body);
        let len = full.chars().count();
        if used + len <= cap {
            used += len;
            out.blocks.push(full);
            out.shown.push(index);
        } else if out.shown.is_empty() {
            let note = format!("\n[Cut to fit; the full text is in `{}`.]", item.entry.file);
            let room = cap.saturating_sub(header.chars().count() + 1 + note.chars().count());
            let body: String = item.entry.body.chars().take(room).collect();
            let block = format!("{header}\n{}{note}", body.trim_end());
            used += block.chars().count();
            out.blocks.push(block);
            out.shown.push(index);
            out.cut.push(index);
        } else {
            out.deferred.push(index);
        }
    }
    out
}

/// The first lines of an entry's block: where it applies, who owns it, and whether to verify it.
fn header(item: &Served<'_>) -> String {
    let entry = item.entry;
    let scope = entry
        .front
        .scope
        .iter()
        .map(|s| format!("`{s}`"))
        .collect::<Vec<_>>()
        .join(", ");
    let mut text = format!(
        "ACS lore for {scope} ({}, owner {}):",
        entry.id(),
        entry.front.owner
    );
    if let Some(note) = item.staleness.and_then(Staleness::verify_note) {
        text.push('\n');
        text.push_str(&note);
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::format::LoreEntry;

    fn entry(id: &str, scope: &str, body: &str) -> Result<LoreEntry, crate::LoreProblem> {
        let text = format!(
            "+++\nid = \"{id}\"\nscope = [\"{scope}\"]\nowner = \"sara@example.com\"\n\
             written_against = \"a1b2c3d\"\n+++\n{body}\n"
        );
        LoreEntry::parse(&format!(".acs/policy/lore/{id}.md"), &text, false)
    }

    fn set() -> LoreSet {
        LoreSet::from_parsed(vec![
            entry("lore_all", "**", "Everything."),
            entry("lore_pay", "payments/**", "Integer cents."),
            entry("lore_api", "payments/api/**", "Idempotency keys."),
            entry("lore_docs", "docs/**", "Plain English."),
        ])
    }

    fn ids(found: &[Match<'_>]) -> Vec<String> {
        found.iter().map(|m| m.entry.id().to_string()).collect()
    }

    #[test]
    fn most_specific_first() {
        let set = set();
        let found = resolve(&set, &["payments/api/refund.ts"]);
        assert_eq!(ids(&found), ["lore_api", "lore_pay", "lore_all"]);
        let found = resolve(&set, &["docs/a.md", "payments/x.ts"]);
        assert_eq!(ids(&found), ["lore_pay", "lore_docs", "lore_all"]);
        assert_eq!(found[1].path, "docs/a.md");
        assert!(resolve(&LoreSet::default(), &["a"]).is_empty());
    }

    #[test]
    fn render_fits_the_cap_and_defers_the_rest() {
        let set = LoreSet::from_parsed(vec![
            entry("lore_a", "a/**", &"x".repeat(100)),
            entry("lore_b", "a/**", &"y".repeat(400)),
            entry("lore_c", "a/**", "short"),
        ]);
        let items: Vec<Served<'_>> = set
            .entries()
            .iter()
            .map(|entry| Served {
                entry,
                staleness: None,
            })
            .collect();
        let out = render(&items, 300);
        assert_eq!(out.shown, [0, 2]);
        assert_eq!(out.deferred, [1]);
        assert!(out.cut.is_empty());
        let total: usize = out.blocks.iter().map(|b| b.chars().count()).sum();
        assert!(total <= 300, "{total}");
        assert!(out.blocks[0].starts_with("ACS lore for `a/**` (lore_a, owner sara@example.com):"));
    }

    #[test]
    fn a_first_entry_too_long_is_cut_with_a_pointer() {
        let set = LoreSet::from_parsed(vec![entry("lore_a", "a/**", &"x".repeat(1000))]);
        let items = [Served {
            entry: &set.entries()[0],
            staleness: None,
        }];
        let out = render(&items, 200);
        assert_eq!(out.cut, [0]);
        assert!(out.blocks[0].chars().count() <= 200);
        assert!(out.blocks[0].ends_with("`.acs/policy/lore/lore_a.md`.]"));
    }

    #[test]
    fn stale_entries_carry_the_verify_note() {
        let set = LoreSet::from_parsed(vec![entry("lore_a", "a/**", "Body.")]);
        let stale = Staleness::Stale {
            percent: 42,
            since: "a1b2c3d".to_owned(),
        };
        let items = [Served {
            entry: &set.entries()[0],
            staleness: Some(&stale),
        }];
        let out = render(&items, 4000);
        assert!(
            out.blocks[0].contains("may be out of date"),
            "{}",
            out.blocks[0]
        );
        assert!(out.blocks[0].contains("42%"));
    }
}
