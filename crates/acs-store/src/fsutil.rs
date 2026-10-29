//! Small filesystem helpers shared by the store: owner-only directories and files, and atomic
//! writes.
//!
//! Permission model (see decision record 0003): on Unix every directory ACS manages is created
//! with mode `0o700` and files with `0o600`, and a managed directory that already exists with
//! group or other bits set is tightened. On other platforms the helpers create ordinary
//! directories and files and rely on the ACL inherited from the parent.

use std::fs::{self, DirBuilder, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// Creates `path` and any missing parents as owner-only directories.
///
/// Missing components are created with mode `0o700` on Unix. If `path` itself already exists
/// with group or other permission bits set, they are removed. Existing parents are left alone:
/// the caller decides which directories ACS owns by calling this on each of them.
///
/// # Errors
///
/// Returns the underlying I/O error, for example when `path` exists and is not a directory or
/// when a directory owned by another user cannot be tightened.
pub fn create_private_dir_all(path: &Path) -> io::Result<()> {
    let mut builder = DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(path)?;
    tighten_dir(path)
}

/// Removes group and other permission bits from the directory `path` (Unix only; a no-op
/// elsewhere).
#[cfg(unix)]
fn tighten_dir(path: &Path) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mode = fs::metadata(path)?.permissions().mode();
    if mode & 0o077 != 0 {
        fs::set_permissions(path, fs::Permissions::from_mode((mode & !0o077) | 0o700))?;
    }
    Ok(())
}

#[cfg(not(unix))]
#[allow(clippy::unnecessary_wraps)]
fn tighten_dir(_path: &Path) -> io::Result<()> {
    Ok(())
}

/// Options for creating a new file that only the owner can read and write (mode `0o600` on
/// Unix).
///
/// The returned options have `write` and `create_new` set; callers add `read`, `truncate` and
/// so on as needed. Later work packages use this for `state.db` and snapshot blobs.
#[must_use]
pub fn private_file_options() -> OpenOptions {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options
}

/// Writes `bytes` to `path` atomically with owner-only permissions.
///
/// The data goes to a uniquely named sibling file which is then renamed over `path`, so a
/// reader sees either the old content or the new content, never a partial file. The data is not
/// fsynced, so this protects against concurrent readers and crashes of the process, not against
/// power loss. The parent directory must exist.
///
/// # Errors
///
/// Returns the underlying I/O error. A failed write leaves no temporary file behind.
pub fn write_private_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let name = path.file_name().map_or_else(
        || "file".to_owned(),
        |name| name.to_string_lossy().into_owned(),
    );
    let tmp: PathBuf = parent.join(format!(
        ".{name}.tmp-{}-{}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    let result = (|| {
        let mut file: File = private_file_options().open(&tmp)?;
        file.write_all(bytes)?;
        drop(file);
        fs::rename(&tmp, path)
    })();
    if result.is_err() {
        // Best effort: the original error is the one worth reporting.
        let _ = fs::remove_file(&tmp);
    }
    result
}
