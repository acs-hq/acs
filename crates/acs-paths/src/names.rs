//! Small pure helpers for path components: case folding, Windows name rules, and on-disk case
//! lookup. Everything here is testable on every platform.

use std::borrow::Cow;
use std::ffi::{OsStr, OsString};
use std::fs;
use std::path::Path;

/// Fold a component for case-insensitive comparison.
pub(crate) fn fold(name: &OsStr) -> String {
    name.to_string_lossy().to_lowercase()
}

/// Whether two components are equal, optionally ignoring case.
pub(crate) fn names_eq(a: &OsStr, b: &OsStr, case_insensitive: bool) -> bool {
    a == b || (case_insensitive && fold(a) == fold(b))
}

/// Apply the Win32 name rules that make two spellings name one file: a trailing run of dots and
/// spaces is dropped, and an NTFS alternate data stream suffix (`name:stream`, `name::$DATA`) is
/// ignored. Returns the name unchanged when the rules do not apply or would leave it empty.
pub(crate) fn windows_name(name: &str) -> Cow<'_, str> {
    if name == "." || name == ".." {
        return Cow::Borrowed(name);
    }
    let base = name.split_once(':').map_or(name, |(base, _)| base);
    let trimmed = base.trim_end_matches(['.', ' ']);
    if trimmed.is_empty() || trimmed == name {
        Cow::Borrowed(name)
    } else {
        Cow::Owned(trimmed.to_owned())
    }
}

/// Whether `name` is a Windows reserved device name (`NUL`, `CON`, `COM1`, `LPT1`, ...), with or
/// without an extension. Such a name refers to a device in every directory.
pub(crate) fn is_windows_device(name: &str) -> bool {
    let stem = name.split('.').next().unwrap_or(name).trim_end_matches(' ');
    let upper = stem.to_ascii_uppercase();
    matches!(
        upper.as_str(),
        "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$"
    ) || (upper.len() == 4
        && (upper.starts_with("COM") || upper.starts_with("LPT"))
        && upper.as_bytes()[3].is_ascii_digit()
        && upper.as_bytes()[3] != b'0')
}

/// Find the name `name` has in `dir` on disk, ignoring case: the exact spelling when it exists
/// (a case-sensitive directory can hold several), else the first case-insensitive match in
/// sorted order. `None` when nothing matches or the directory cannot be read.
pub(crate) fn on_disk_name(dir: &Path, name: &OsStr) -> Option<OsString> {
    let wanted = fold(name);
    let mut best: Option<OsString> = None;
    for entry in fs::read_dir(dir).ok()?.flatten() {
        let found = entry.file_name();
        if found == name {
            return Some(found);
        }
        if fold(&found) == wanted && best.as_ref().is_none_or(|b| found < *b) {
            best = Some(found);
        }
    }
    best
}

/// Swap the case of every ASCII letter, or `None` if there is none to swap.
pub(crate) fn flip_case(name: &OsStr) -> Option<OsString> {
    let text = name.to_str()?;
    let flipped: String = text
        .chars()
        .map(|c| {
            if c.is_ascii_lowercase() {
                c.to_ascii_uppercase()
            } else {
                c.to_ascii_lowercase()
            }
        })
        .collect();
    (flipped != text).then(|| OsString::from(flipped))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_name_rules() {
        for (given, want) in [
            ("a.txt", "a.txt"),
            ("a.txt.", "a.txt"),
            ("a.txt . .", "a.txt"),
            ("a.txt:stream", "a.txt"),
            ("a.txt::$DATA", "a.txt"),
            (".gitignore", ".gitignore"),
            ("...", "..."),
            (".", "."),
            ("..", ".."),
            (":$DATA", ":$DATA"),
        ] {
            assert_eq!(windows_name(given), want, "{given:?}");
        }
    }

    #[test]
    fn device_names() {
        for yes in ["NUL", "nul", "con.txt", "COM1", "lpt9", "AUX", "CONOUT$"] {
            assert!(is_windows_device(yes), "{yes}");
        }
        for no in ["null", "COM0", "COM10", "console", "a.txt", "com"] {
            assert!(!is_windows_device(no), "{no}");
        }
    }

    #[test]
    fn flips_ascii_case_only() {
        assert_eq!(flip_case(OsStr::new("Repo-1")), Some("rEPO-1".into()));
        assert_eq!(flip_case(OsStr::new("123")), None);
    }

    #[test]
    fn on_disk_name_prefers_exact_then_first_fold() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("Readme.md"), "").unwrap();
        assert_eq!(
            on_disk_name(dir.path(), OsStr::new("readme.MD")),
            Some("Readme.md".into())
        );
        assert_eq!(
            on_disk_name(dir.path(), OsStr::new("Readme.md")),
            Some("Readme.md".into())
        );
        assert_eq!(on_disk_name(dir.path(), OsStr::new("other")), None);
    }
}
