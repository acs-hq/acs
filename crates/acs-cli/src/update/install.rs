//! How this `acs` was installed, which decides whether it replaces itself (decision 0054).
//!
//! - **Installer** (Windows only): the PowerShell installer that `dist` generates wrote a receipt
//!   (`%LOCALAPPDATA%\acs-cli\acs-cli-receipt.json`, or `%XDG_CONFIG_HOME%\acs-cli\` when that is
//!   set) and its binary directory is the directory of the running `acs.exe`.
//! - **Scoop**: the running file is under `scoop\apps\`. Notice only.
//! - **Source**: a build without the cargo feature `self-update` (`cargo install --path`).
//! - **Other**: everything else, Windows installs without a matching receipt included.

use std::path::{Component, Path, PathBuf};

use serde::Deserialize;

/// The file name of the installer's receipt.
pub const RECEIPT_FILE: &str = "acs-cli-receipt.json";

/// How this `acs` got here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Method {
    /// The installer's receipt names this file's directory.
    Installer,
    /// Under `scoop\apps\`.
    Scoop,
    /// A build that does not update itself (the cargo feature `self-update` is off).
    Source,
    /// Anything else.
    Other,
}

impl Method {
    /// A short name for people and JSON.
    #[must_use]
    pub fn label(&self) -> &'static str {
        match self {
            Self::Installer => "the installer",
            Self::Scoop => "Scoop",
            Self::Source => "a source build",
            Self::Other => "an unknown method",
        }
    }

    /// The command that updates an install that does not update itself, if there is one to name.
    #[must_use]
    pub fn manual_update_command(&self) -> Option<String> {
        match self {
            Self::Installer | Self::Source => None,
            Self::Scoop => Some("scoop update acs".to_owned()),
            Self::Other => Some(installer_line().to_owned()),
        }
    }
}

/// The line from the README that installs the newest release.
#[must_use]
pub fn installer_line() -> &'static str {
    if cfg!(windows) {
        "irm https://github.com/acs-hq/acs/releases/latest/download/acs-cli-installer.ps1 | iex"
    } else {
        "curl --proto '=https' --tlsv1.2 -LsSf https://github.com/acs-hq/acs/releases/latest/download/acs-cli-installer.sh | sh"
    }
}

#[derive(Deserialize)]
struct Receipt {
    install_prefix: String,
    install_layout: Option<String>,
}

/// The directory the installer put `acs` in, from the receipt's text: `<prefix>` for the `flat`
/// layout, `<prefix>\bin` for `cargo-home`, `hierarchical` and any other.
#[must_use]
pub fn receipt_bin_dir(receipt_json: &str) -> Option<PathBuf> {
    let text = receipt_json.trim_start_matches('\u{feff}').trim();
    let receipt: Receipt = serde_json::from_str(text).ok()?;
    if receipt.install_prefix.is_empty() {
        return None;
    }
    let prefix = PathBuf::from(receipt.install_prefix);
    Some(match receipt.install_layout.as_deref() {
        Some("flat") => prefix,
        _ => prefix.join("bin"),
    })
}

/// A path as a comparable string: links resolved when it exists, `\` as `/`, no verbatim prefix,
/// no trailing slash, lowercase.
fn comparable(path: &Path) -> String {
    let resolved = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let text = resolved.to_string_lossy().replace('\\', "/");
    let text = text.strip_prefix("//?/").unwrap_or(&text);
    text.trim_end_matches('/').to_lowercase()
}

/// Whether the receipt in `receipt_json` names `exe_dir` as the installer's binary directory,
/// compared case-insensitively after canonicalising.
#[must_use]
pub fn receipt_matches(receipt_json: &str, exe_dir: &Path) -> bool {
    receipt_bin_dir(receipt_json).is_some_and(|dir| comparable(&dir) == comparable(exe_dir))
}

/// Whether `exe` lies under a `scoop\apps\` directory.
#[must_use]
pub fn is_scoop(exe: &Path) -> bool {
    let parts: Vec<String> = exe
        .components()
        .filter_map(|c| match c {
            Component::Normal(name) => Some(name.to_string_lossy().to_lowercase()),
            _ => None,
        })
        .collect();
    parts.windows(2).any(|w| w[0] == "scoop" && w[1] == "apps")
}

/// The folders that can hold the installer's receipt, most specific first:
/// `%XDG_CONFIG_HOME%\acs-cli` when set, then `%LOCALAPPDATA%\acs-cli`.
#[must_use]
pub fn receipt_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    for var in ["XDG_CONFIG_HOME", "LOCALAPPDATA"] {
        if let Some(base) = std::env::var_os(var).filter(|v| !v.is_empty()) {
            dirs.push(PathBuf::from(base).join("acs-cli"));
        }
    }
    dirs
}

/// Decides the method for the file `exe`, given the receipt folders to look in.
///
/// `enabled` is whether this build acts at all (the feature or the debug override); `windows`
/// whether replacing itself is possible here (installer installs exist on Windows only).
#[must_use]
pub fn detect(exe: &Path, receipt_dirs: &[PathBuf], enabled: bool, windows: bool) -> Method {
    if !enabled {
        return Method::Source;
    }
    if is_scoop(exe) {
        return Method::Scoop;
    }
    if windows && let Some(dir) = exe.parent() {
        let matched = receipt_dirs.iter().any(|folder| {
            std::fs::read_to_string(folder.join(RECEIPT_FILE))
                .is_ok_and(|text| receipt_matches(&text, dir))
        });
        if matched {
            return Method::Installer;
        }
    }
    Method::Other
}

/// The method of the running process.
#[must_use]
pub fn current(enabled: bool) -> Method {
    match std::env::current_exe() {
        Ok(exe) => detect(&exe, &receipt_dirs(), enabled, cfg!(windows)),
        Err(_) => {
            if enabled {
                Method::Other
            } else {
                Method::Source
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn receipt(prefix: &str, layout: &str) -> String {
        format!(
            "{{\"binaries\":[\"acs\"],\"install_layout\":\"{layout}\",\"install_prefix\":{},\
             \"modify_path\":true,\"source\":{{\"app_name\":\"acs-cli\"}},\"version\":\"1.0.0\"}}\r\n",
            serde_json::to_string(prefix).unwrap()
        )
    }

    #[test]
    fn the_layouts_name_their_binary_directory() {
        let dir = |layout| receipt_bin_dir(&receipt("C:\\Users\\x\\.cargo", layout)).unwrap();
        assert_eq!(
            dir("cargo-home"),
            Path::new("C:\\Users\\x\\.cargo").join("bin")
        );
        assert_eq!(
            dir("hierarchical"),
            Path::new("C:\\Users\\x\\.cargo").join("bin")
        );
        assert_eq!(dir("flat"), PathBuf::from("C:\\Users\\x\\.cargo"));
        assert!(receipt_bin_dir("not json").is_none());
        assert!(receipt_bin_dir(&receipt("", "flat")).is_none());
        let bom = format!("\u{feff}{}", receipt("D:\\a", "flat"));
        assert_eq!(receipt_bin_dir(&bom).unwrap(), PathBuf::from("D:\\a"));
    }

    #[test]
    fn a_receipt_matches_its_directory_whatever_the_case_and_separators() {
        let dir = tempfile::tempdir().unwrap();
        let prefix = dir.path().join("Cargo");
        std::fs::create_dir_all(prefix.join("bin")).unwrap();
        let text = receipt(&prefix.to_string_lossy().to_uppercase(), "cargo-home");
        assert!(receipt_matches(&text, &prefix.join("bin")));
        let flat = receipt(&prefix.join("bin").to_string_lossy().to_lowercase(), "flat");
        assert!(receipt_matches(&flat, &prefix.join("bin")));
        assert!(!receipt_matches(&text, &prefix));
        assert!(!receipt_matches(&text, &dir.path().join("other")));
    }

    #[test]
    fn a_receipt_that_names_missing_directories_still_compares_by_text() {
        let text = receipt("C:\\Users\\X\\.cargo", "cargo-home");
        assert!(receipt_matches(
            &text,
            Path::new("c:\\users\\x\\.CARGO\\bin")
        ));
        assert!(!receipt_matches(
            &text,
            Path::new("c:\\users\\x\\.cargo\\bin2")
        ));
    }

    #[test]
    fn scoop_is_recognised_by_its_apps_folder() {
        assert!(is_scoop(Path::new(
            "C:\\Users\\x\\scoop\\apps\\acs\\current\\acs.exe"
        )));
        assert!(is_scoop(Path::new(
            "C:\\Users\\x\\SCOOP\\Apps\\acs\\1.0.0\\acs.exe"
        )));
        assert!(!is_scoop(Path::new("C:\\Users\\x\\scoop\\shims\\acs.exe")));
        assert!(!is_scoop(Path::new("C:\\Users\\x\\.cargo\\bin\\acs.exe")));
    }

    #[test]
    fn detect_applies_the_rules_in_order() {
        let tmp = tempfile::tempdir().unwrap();
        let bin = tmp.path().join("prefix").join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        let folder = tmp.path().join("acs-cli");
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::write(
            folder.join(RECEIPT_FILE),
            receipt(&tmp.path().join("prefix").to_string_lossy(), "cargo-home"),
        )
        .unwrap();
        let exe = bin.join("acs.exe");
        let dirs = [folder.clone()];
        assert_eq!(detect(&exe, &dirs, true, true), Method::Installer);
        assert_eq!(detect(&exe, &dirs, false, true), Method::Source);
        assert_eq!(detect(&exe, &dirs, true, false), Method::Other);
        assert_eq!(detect(&exe, &[], true, true), Method::Other);
        let elsewhere = tmp.path().join("elsewhere").join("acs.exe");
        assert_eq!(detect(&elsewhere, &dirs, true, true), Method::Other);
        let scoop = tmp
            .path()
            .join("scoop")
            .join("apps")
            .join("acs")
            .join("acs.exe");
        assert_eq!(detect(&scoop, &dirs, true, true), Method::Scoop);
    }

    #[test]
    fn notice_only_installs_name_their_update_command() {
        assert_eq!(
            Method::Scoop.manual_update_command().as_deref(),
            Some("scoop update acs")
        );
        assert!(
            Method::Other
                .manual_update_command()
                .unwrap()
                .contains("installer")
        );
        assert!(Method::Installer.manual_update_command().is_none());
        assert!(Method::Source.manual_update_command().is_none());
    }
}
