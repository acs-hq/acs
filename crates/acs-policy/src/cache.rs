//! The compiled-policy cache and the fallback chain of SPEC section 11.
//!
//! The cache is one file, `policy.json`, in a directory the caller supplies (the repository's
//! `cache/` directory under `ACS_HOME`). It holds the *last valid* policy: the validated source
//! structs plus the hash of the policy files they were loaded from. Globs are cheap to compile
//! (see [`crate::glob`]), so the cache stores the source rather than a compiled form, and one
//! entry can serve both case-sensitivity settings.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use acs_core::ContentHash;
use serde::{Deserialize, Serialize};

use crate::LoadOptions;
use crate::compiled::CompiledPolicy;
use crate::error::PolicyError;
use crate::files::PolicyFiles;
use crate::load::RawFiles;

/// The cache file's name inside the cache directory.
pub const CACHE_FILE: &str = "policy.json";

/// Bump when the cache layout or the built-in defaults change; older entries are then ignored.
const CACHE_FORMAT: u32 = 1;

/// A cache entry as read back.
#[derive(Deserialize)]
struct Entry {
    format: u32,
    version: String,
    hash: ContentHash,
    files: PolicyFiles,
}

/// A cache entry as written.
#[derive(Serialize)]
struct EntryRef<'a> {
    format: u32,
    version: &'a str,
    hash: &'a ContentHash,
    files: &'a PolicyFiles,
}

/// Where the policy returned by [`load_cached`] came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Origin {
    /// Parsed from the policy files (the cache was missing or stale) and cached.
    Fresh,
    /// The policy files are unchanged since the cache entry was written.
    Cache,
    /// The policy files are invalid; the last valid policy from the cache is used.
    LastValid,
    /// The policy files are invalid and nothing is cached; only the built-in defaults apply.
    Defaults,
}

/// The result of [`load_cached`]: always a usable policy.
#[derive(Debug)]
pub struct LoadOutcome {
    /// The policy to enforce.
    pub policy: CompiledPolicy,
    /// Where it came from.
    pub origin: Origin,
    /// Things the user should hear about: why a fallback was used, a cache that could not be
    /// written, an empty team. The caller decides how to show them.
    pub warnings: Vec<String>,
}

/// Loads the policy in `policy_dir` through the cache in `cache_dir`.
///
/// 1. The three policy files are read and hashed. If the cache entry carries the same hash, the
///    policy is compiled from it without parsing TOML ([`Origin::Cache`]).
/// 2. Otherwise the files are parsed, validated and compiled, and the result replaces the cache
///    entry ([`Origin::Fresh`]). A cache that cannot be written produces a warning only.
/// 3. If the files are invalid or unreadable, the cached last valid policy is used
///    ([`Origin::LastValid`]) with a warning that quotes the error; with no usable cache, the
///    built-in defaults are used ([`Origin::Defaults`]) with a warning (SPEC section 11).
///
/// A corrupt or incompatible cache is ignored (corruption adds a warning). Nothing is printed.
#[must_use]
pub fn load_cached(policy_dir: &Path, cache_dir: &Path, opts: LoadOptions) -> LoadOutcome {
    let ci = opts.case_insensitive;
    let (cached, mut warnings) = read_entry(cache_dir);
    let raw = RawFiles::read(policy_dir);

    let error = match raw.and_then(|raw| fresh(&raw, cached.as_ref(), ci)) {
        Ok((policy, origin)) => {
            if origin == Origin::Fresh
                && let Err(err) = write_entry(cache_dir, &policy)
            {
                warnings.push(format!(
                    "could not write the policy cache in {}: {err}; policy files will be parsed on every call",
                    cache_dir.display()
                ));
            }
            warnings.extend(policy.warnings());
            return LoadOutcome {
                policy,
                origin,
                warnings,
            };
        }
        Err(error) => error,
    };

    if let Some(entry) = cached
        && let Ok(policy) = CompiledPolicy::build(entry.files, entry.hash, ci)
    {
        warnings.push(format!(
            "{error}\nUsing the last valid policy (hash {}) until this is fixed.",
            short(policy.hash())
        ));
        return LoadOutcome {
            policy,
            origin: Origin::LastValid,
            warnings,
        };
    }
    warnings.push(format!(
        "{error}\nNo earlier valid policy is cached, so only the built-in default guards apply until this is fixed."
    ));
    LoadOutcome {
        policy: CompiledPolicy::builtin(ci),
        origin: Origin::Defaults,
        warnings,
    }
}

/// Compiles the policy for `raw`, from the cache entry when its hash matches.
fn fresh(
    raw: &RawFiles,
    cached: Option<&Entry>,
    ci: bool,
) -> Result<(CompiledPolicy, Origin), PolicyError> {
    let hash = raw.hash();
    if let Some(entry) = cached
        && entry.hash == hash
        && let Ok(policy) = CompiledPolicy::build(entry.files.clone(), hash, ci)
    {
        return Ok((policy, Origin::Cache));
    }
    Ok((raw.compile(ci)?, Origin::Fresh))
}

fn short(hash: &ContentHash) -> &str {
    &hash.as_str()[..12]
}

/// Reads the cache entry, if there is a usable one.
fn read_entry(cache_dir: &Path) -> (Option<Entry>, Vec<String>) {
    let path = cache_dir.join(CACHE_FILE);
    let bytes = match fs::read(&path) {
        Ok(bytes) => bytes,
        Err(err)
            if matches!(
                err.kind(),
                io::ErrorKind::NotFound | io::ErrorKind::NotADirectory
            ) =>
        {
            return (None, Vec::new());
        }
        Err(err) => return (None, vec![ignoring(&path, &err.to_string())]),
    };
    match serde_json::from_slice::<Entry>(&bytes) {
        Ok(entry) if entry.format == CACHE_FORMAT && entry.version == env!("CARGO_PKG_VERSION") => {
            (Some(entry), Vec::new())
        }
        // Written by another build of ACS: not corruption, just stale.
        Ok(_) => (None, Vec::new()),
        Err(err) => (None, vec![ignoring(&path, &err.to_string())]),
    }
}

fn ignoring(path: &Path, why: &str) -> String {
    format!(
        "ignoring the unreadable policy cache {} ({why}); it is rebuilt on the next valid load",
        path.display()
    )
}

/// Writes the cache entry atomically: a temporary sibling file renamed over the cache file, so a
/// reader sees the old entry or the new one, never a partial file.
fn write_entry(cache_dir: &Path, policy: &CompiledPolicy) -> io::Result<()> {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let entry = EntryRef {
        format: CACHE_FORMAT,
        version: env!("CARGO_PKG_VERSION"),
        hash: policy.hash(),
        files: policy.files(),
    };
    let bytes = serde_json::to_vec(&entry)?;
    fs::create_dir_all(cache_dir)?;
    let tmp: PathBuf = cache_dir.join(format!(
        ".{CACHE_FILE}.tmp-{}-{}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    let result = (|| {
        let mut file: File = private_file().open(&tmp)?;
        file.write_all(&bytes)?;
        drop(file);
        fs::rename(&tmp, cache_dir.join(CACHE_FILE))
    })();
    if result.is_err() {
        // Best effort: the original error is the one worth reporting.
        let _ = fs::remove_file(&tmp);
    }
    result
}

/// Options for creating a new file readable only by its owner (Unix mode `0o600`).
fn private_file() -> OpenOptions {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options
}

/// The hash of the policy files in `policy_dir` (see [`CompiledPolicy::hash`]), without parsing
/// them.
///
/// # Errors
///
/// Returns [`PolicyError::Io`] if a policy file exists but cannot be read.
pub fn policy_hash(policy_dir: &Path) -> Result<ContentHash, PolicyError> {
    Ok(RawFiles::read(policy_dir)?.hash())
}
