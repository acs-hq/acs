//! Downloading and unpacking with the programs Windows ships: `curl.exe` and `tar.exe` (bsdtar,
//! which reads zip), both started by their absolute path under `%SystemRoot%\System32`, never
//! looked up on `PATH` (decision 0054). `acs` has no HTTP client of its own.
//!
//! Elsewhere (a check that only records the newest version) `curl` is `/usr/bin/curl`.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// How long a listing may take, in seconds.
pub const LISTING_SECS: u32 = 30;
/// How long a download may take, in seconds.
pub const DOWNLOAD_SECS: u32 = 300;

/// The absolute path of `curl`.
#[must_use]
pub fn curl_path() -> PathBuf {
    system32("curl.exe", "/usr/bin/curl")
}

/// The absolute path of `tar`.
#[must_use]
pub fn tar_path() -> PathBuf {
    system32("tar.exe", "/usr/bin/tar")
}

fn system32(windows_name: &str, other: &str) -> PathBuf {
    if cfg!(windows) {
        let root = std::env::var_os("SystemRoot")
            .filter(|v| !v.is_empty())
            .map_or_else(|| PathBuf::from(r"C:\Windows"), PathBuf::from);
        root.join("System32").join(windows_name)
    } else {
        PathBuf::from(other)
    }
}

/// The `curl` arguments that fetch `url` into `out`. Only `https` is allowed, redirects included,
/// unless the address itself is `http://127.0.0.1` (a debug build's test server).
#[must_use]
pub fn curl_args(url: &str, out: &Path, max_secs: u32) -> Vec<String> {
    let local = url.starts_with("http://127.0.0.1:");
    let mut args: Vec<String> = Vec::new();
    let mut add = |items: &[&str]| args.extend(items.iter().map(|s| (*s).to_owned()));
    if local {
        add(&["--proto", "=http", "--proto-redir", "=http"]);
    } else {
        add(&["--proto", "=https", "--proto-redir", "=https", "--tlsv1.2"]);
    }
    add(&[
        "--fail",
        "--silent",
        "--show-error",
        "--location",
        "--max-time",
        &max_secs.to_string(),
        "--user-agent",
        "acs-update",
        "--output",
    ]);
    args.push(out.to_string_lossy().into_owned());
    args.push(url.to_owned());
    args
}

/// Downloads `url` to `out`.
///
/// # Errors
///
/// What `curl` said (or why it did not start).
pub fn download(url: &str, out: &Path, max_secs: u32) -> Result<(), String> {
    let program = curl_path();
    let output = Command::new(&program)
        .args(curl_args(url, out, max_secs))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .map_err(|err| format!("cannot run {}: {err}", program.display()))?;
    if output.status.success() {
        return Ok(());
    }
    let said = String::from_utf8_lossy(&output.stderr);
    let said = said.trim();
    Err(if said.is_empty() {
        format!("download of {url} failed ({})", output.status)
    } else {
        format!("download of {url} failed: {said}")
    })
}

/// Unpacks the archive `archive` into the existing directory `into`.
///
/// # Errors
///
/// What `tar` said (or why it did not start).
pub fn unpack(archive: &Path, into: &Path) -> Result<(), String> {
    let program = tar_path();
    let output = Command::new(&program)
        .arg("-xf")
        .arg(archive)
        .arg("-C")
        .arg(into)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .map_err(|err| format!("cannot run {}: {err}", program.display()))?;
    if output.status.success() {
        return Ok(());
    }
    Err(format!(
        "unpacking failed: {}",
        String::from_utf8_lossy(&output.stderr).trim()
    ))
}

/// The unpacked `exe_name` in `dir`: at the top, or one folder down (how `dist` lays out an
/// archive that holds a directory).
#[must_use]
pub fn find_unpacked(dir: &Path, exe_name: &str) -> Option<PathBuf> {
    let top = dir.join(exe_name);
    if top.is_file() {
        return Some(top);
    }
    std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .map(|entry| entry.path().join(exe_name))
        .find(|candidate| candidate.is_file())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn https_downloads_pin_the_protocol_and_tls() {
        let args = curl_args("https://github.com/x", Path::new("out.bin"), 30);
        let line = args.join(" ");
        assert!(
            line.contains("--proto =https --proto-redir =https --tlsv1.2"),
            "{line}"
        );
        for flag in [
            "--fail",
            "--silent",
            "--show-error",
            "--location",
            "--max-time 30",
        ] {
            assert!(line.contains(flag), "{flag}");
        }
        assert!(
            line.ends_with("--output out.bin https://github.com/x"),
            "{line}"
        );
    }

    #[test]
    fn only_a_local_test_server_may_use_http() {
        let local = curl_args("http://127.0.0.1:9/x", Path::new("o"), 5).join(" ");
        assert!(
            local.contains("--proto =http ") && !local.contains("=https"),
            "{local}"
        );
        let remote = curl_args("http://example.com/x", Path::new("o"), 5).join(" ");
        assert!(remote.contains("--proto =https"), "{remote}");
    }

    #[test]
    fn the_programs_have_absolute_paths() {
        assert!(curl_path().is_absolute());
        assert!(tar_path().is_absolute());
        if cfg!(windows) {
            assert!(
                curl_path()
                    .to_string_lossy()
                    .to_lowercase()
                    .contains("system32")
            );
        }
    }

    #[test]
    fn the_executable_is_found_at_the_top_or_one_folder_down() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(find_unpacked(tmp.path(), "acs.exe").is_none());
        let nested = tmp.path().join("acs-cli-x86_64-pc-windows-msvc");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::write(nested.join("acs.exe"), "x").unwrap();
        assert_eq!(
            find_unpacked(tmp.path(), "acs.exe"),
            Some(nested.join("acs.exe"))
        );
        std::fs::write(tmp.path().join("acs.exe"), "y").unwrap();
        assert_eq!(
            find_unpacked(tmp.path(), "acs.exe"),
            Some(tmp.path().join("acs.exe"))
        );
    }
}
