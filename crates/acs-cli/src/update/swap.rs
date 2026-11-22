//! Replacing the running file (Windows: a running `.exe` can be renamed but not overwritten).
//!
//! The steps of decision 0054: copy the new file next to the running one as `acs-new.exe`, run
//! `acs-new.exe --version` (it must start and print the candidate version; an antivirus block
//! shows here), rename the running file to `acs.exe.old-<unix seconds>`, rename `acs-new.exe` to
//! `acs.exe`, and undo the first rename if the second fails. Older `acs.exe.old-*` files are
//! deleted at the start of every later check; one still in use stays until a later check.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// The name the new file has next to the running one until the swap.
pub const NEW_NAME: &str = "acs-new.exe";

/// The name the running file gets: `<name>.old-<unix seconds>`.
#[must_use]
pub fn old_path(current: &Path, stamp: i64) -> PathBuf {
    let mut name = current.file_name().unwrap_or_default().to_os_string();
    name.push(format!(".old-{stamp}"));
    current.with_file_name(name)
}

/// Copies `source` to `acs-new.exe` in the directory of `current`.
///
/// # Errors
///
/// The underlying I/O error, with the path in the text.
pub fn place_new(current: &Path, source: &Path) -> Result<PathBuf, String> {
    let target = current.with_file_name(NEW_NAME);
    fs::copy(source, &target).map_err(|err| format!("cannot write {}: {err}", target.display()))?;
    Ok(target)
}

/// Runs `<candidate> --version` and checks that it prints `version` as its last word.
///
/// # Errors
///
/// Why the program did not start, did not exit cleanly, or printed something else.
pub fn try_version(candidate: &Path, version: &str) -> Result<(), String> {
    let output = Command::new(candidate)
        .arg("--version")
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .map_err(|err| {
            format!(
                "the new version does not start ({}): {err}",
                candidate.display()
            )
        })?;
    if !output.status.success() {
        return Err(format!(
            "the new version does not start ({} exited with {})",
            candidate.display(),
            output.status
        ));
    }
    let printed = String::from_utf8_lossy(&output.stdout);
    if printed.split_whitespace().last() == Some(version) {
        Ok(())
    } else {
        Err(format!(
            "the new version reports `{}`, not {version}",
            printed.trim()
        ))
    }
}

/// Renames `current` to `<current>.old-<stamp>`, then `new` to `current`; if the second rename
/// fails the first is undone and `current` is as it was. Returns the old file's new path.
///
/// # Errors
///
/// The failed rename, in words.
pub fn swap(current: &Path, new: &Path, stamp: i64) -> Result<PathBuf, String> {
    let old = old_path(current, stamp);
    fs::rename(current, &old)
        .map_err(|err| format!("cannot move {} out of the way: {err}", current.display()))?;
    if let Err(err) = fs::rename(new, current) {
        return Err(match fs::rename(&old, current) {
            Ok(()) => format!("cannot move the new version into place: {err}"),
            Err(undo) => format!(
                "cannot move the new version into place ({err}) and cannot put {} back ({undo}); \
                 rename it to {} by hand",
                old.display(),
                current.display()
            ),
        });
    }
    Ok(old)
}

/// Deletes the files a past update left next to `current` (`<name>.old-*`, and a stray
/// `acs-new.exe`). Failures are ignored: a file still in use goes at a later check.
pub fn remove_old(current: &Path) {
    let Some(dir) = current.parent() else { return };
    let Some(name) = current
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
    else {
        return;
    };
    let prefix = format!("{name}.old-");
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let file = entry.file_name().to_string_lossy().into_owned();
        if file.starts_with(&prefix) || file == NEW_NAME {
            let _ = fs::remove_file(entry.path());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_old_file_is_named_with_the_time() {
        assert_eq!(
            old_path(Path::new("C:\\bin\\acs.exe"), 1_700_000_000),
            PathBuf::from("C:\\bin\\acs.exe.old-1700000000")
        );
    }

    #[test]
    fn the_swap_moves_the_running_file_aside_and_the_new_one_in() {
        let tmp = tempfile::tempdir().unwrap();
        let current = tmp.path().join("acs.exe");
        let new = tmp.path().join(NEW_NAME);
        fs::write(&current, "old").unwrap();
        fs::write(&new, "new").unwrap();
        let old = swap(&current, &new, 42).unwrap();
        assert_eq!(fs::read_to_string(&current).unwrap(), "new");
        assert_eq!(fs::read_to_string(&old).unwrap(), "old");
        assert!(old.ends_with("acs.exe.old-42"));
        assert!(!new.exists());
    }

    #[test]
    fn a_failed_second_rename_puts_the_first_back() {
        let tmp = tempfile::tempdir().unwrap();
        let current = tmp.path().join("acs.exe");
        fs::write(&current, "old").unwrap();
        let missing = tmp.path().join(NEW_NAME);
        let err = swap(&current, &missing, 42).unwrap_err();
        assert!(err.contains("new version"), "{err}");
        assert_eq!(fs::read_to_string(&current).unwrap(), "old");
        assert!(!old_path(&current, 42).exists());
    }

    #[test]
    fn a_missing_running_file_changes_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        let new = tmp.path().join(NEW_NAME);
        fs::write(&new, "new").unwrap();
        assert!(swap(&tmp.path().join("acs.exe"), &new, 1).is_err());
        assert_eq!(fs::read_to_string(&new).unwrap(), "new");
    }

    #[test]
    fn old_files_and_the_stray_new_file_are_removed_and_nothing_else() {
        let tmp = tempfile::tempdir().unwrap();
        for name in [
            "acs.exe",
            "acs.exe.old-1",
            "acs.exe.old-22",
            NEW_NAME,
            "other.old-1",
        ] {
            fs::write(tmp.path().join(name), "x").unwrap();
        }
        remove_old(&tmp.path().join("acs.exe"));
        let mut left: Vec<_> = fs::read_dir(tmp.path())
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        left.sort();
        assert_eq!(left, ["acs.exe", "other.old-1"]);
    }

    #[test]
    fn place_new_copies_next_to_the_running_file() {
        let tmp = tempfile::tempdir().unwrap();
        let source = tmp.path().join("download.exe");
        fs::write(&source, "bytes").unwrap();
        let current = tmp.path().join("bin").join("acs.exe");
        fs::create_dir_all(current.parent().unwrap()).unwrap();
        let placed = place_new(&current, &source).unwrap();
        assert_eq!(placed, current.with_file_name(NEW_NAME));
        assert_eq!(fs::read_to_string(placed).unwrap(), "bytes");
        assert!(place_new(&current.with_file_name("nodir").join("acs.exe"), &source).is_err());
    }

    #[test]
    fn a_program_that_will_not_start_is_reported() {
        let tmp = tempfile::tempdir().unwrap();
        let err = try_version(&tmp.path().join("nothing.exe"), "1.0.0").unwrap_err();
        assert!(err.contains("does not start"), "{err}");
    }
}
