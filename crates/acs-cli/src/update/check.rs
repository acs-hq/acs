//! One check from start to end, and the session-start spawn.
//!
//! A check lists the releases, records the newest known version, decides (see [`super::version`])
//! and, on a Windows installer install, downloads, verifies, unpacks, tries and swaps. Every
//! failure leaves the running program in place and is written to the state with its time.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use acs_store::AcsHome;

use super::fetch::{self, DOWNLOAD_SECS, LISTING_SECS};
use super::install::{self, Method};
use super::signature;
use super::store::{self, LastError, LastUpdate, Settings, State};
use super::swap;
use super::version::{self, Pick, Release, Rules};
use super::{Overrides, REPOSITORY};

/// Who asked for the check.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// The detached process the session-start hook starts: honours `auto` and the 24 hours.
    Background,
    /// A person ran `acs update`: checks now, and installs a newer major version too.
    Manual,
}

/// Why a newer release was not installed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NoticeKind {
    /// This install does not replace itself (Scoop, no receipt, source build, not Windows).
    NotReplaceable,
    /// A newer major version, which only a person's `acs update` installs.
    NewerMajor,
}

/// What a check did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// It did not run: updating is off, or this build never checks.
    Off(String),
    /// A background check that is not due yet.
    NotDue,
    /// Another check is running.
    Busy,
    /// Nothing newer.
    UpToDate {
        /// The newest release the rules consider, if there is one.
        newest: Option<String>,
    },
    /// The running file was replaced.
    Updated {
        /// The version replaced.
        from: String,
        /// The version installed.
        to: String,
    },
    /// A newer release exists that was not installed.
    Notice {
        /// Its version.
        version: String,
        /// Why not.
        kind: NoticeKind,
        /// The command that updates this install by hand, if there is one to name.
        command: Option<String>,
    },
}

/// The current time in Unix seconds.
#[must_use]
pub fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
}

/// The address of the releases listing.
#[must_use]
pub fn listing_url(overrides: &Overrides) -> String {
    match &overrides.base_url {
        Some(base) => format!("{}/releases", base.trim_end_matches('/')),
        None => format!("https://api.github.com/repos/{REPOSITORY}/releases?per_page=30"),
    }
}

/// The address of a file of release `tag`.
#[must_use]
pub fn asset_url(overrides: &Overrides, tag: &str, name: &str) -> String {
    match &overrides.base_url {
        Some(base) => format!("{}/download/{tag}/{name}", base.trim_end_matches('/')),
        None => format!("https://github.com/{REPOSITORY}/releases/download/{tag}/{name}"),
    }
}

/// The target `acs` was built for, when it can replace itself.
#[must_use]
pub fn target_triple() -> Option<&'static str> {
    if cfg!(all(windows, target_arch = "x86_64")) {
        Some("x86_64-pc-windows-msvc")
    } else if cfg!(all(windows, target_arch = "aarch64")) {
        Some("aarch64-pc-windows-msvc")
    } else {
        None
    }
}

/// Starts a detached `acs update --background` if this build checks for updates, a day has
/// passed since the last check started and `auto` is not false. Never prints, never fails: a
/// check that does not start is tried again at the next session. Builds without the cargo
/// feature `self-update` return before reading anything.
pub fn spawn_if_due(home: Option<&AcsHome>, now: i64) {
    if !Overrides::current().build_enabled() {
        return;
    }
    let Some(home) = home else { return };
    if !State::load(home).due(now) {
        return;
    }
    if !Settings::load(home).is_ok_and(|s| s.auto) {
        return;
    }
    spawn();
}

fn spawn() {
    let Ok(exe) = std::env::current_exe() else {
        return;
    };
    let mut command = Command::new(exe);
    command
        .args(["update", "--background"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    crate::runtime::sync::detach(&mut command);
    acs_os::keep_stdio_from_children();
    let _ = command.spawn();
}

/// Runs one check.
///
/// # Errors
///
/// Why it failed. A failure after the check started is also recorded in the state.
pub fn run(home: &AcsHome, mode: Mode, now: i64) -> Result<Outcome, String> {
    let overrides = Overrides::current();
    if !overrides.build_enabled() && mode == Mode::Background {
        return Ok(Outcome::Off(
            "this build does not check for updates".to_owned(),
        ));
    }
    let settings = match Settings::load(home) {
        Ok(settings) => settings,
        Err(why) if mode == Mode::Background => return Ok(Outcome::Off(why)),
        Err(why) => return Err(why),
    };
    if mode == Mode::Background && !settings.auto {
        return Ok(Outcome::Off("updating is off".to_owned()));
    }
    let _lock = match store::lock(home) {
        Ok(Some(lock)) => lock,
        Ok(None) => return Ok(Outcome::Busy),
        Err(err) => return Err(format!("cannot take the update lock: {err}")),
    };
    let mut state = State::load(home);
    // Two sessions that started together both spawn; the one that gets the lock second finds
    // the first one's start here.
    if mode == Mode::Background && !state.due(now) {
        return Ok(Outcome::NotDue);
    }
    let exe = std::env::current_exe().ok();
    let method = install::current(overrides.build_enabled());
    if method == Method::Installer
        && let Some(exe) = &exe
    {
        swap::remove_old(exe);
    }
    let work = store::dir(home).join(format!("work-{now}-{}", std::process::id()));
    remove_stale_work(&store::dir(home), &work);
    state.check_started_at = Some(now);
    state
        .save(home)
        .map_err(|err| format!("cannot write the update state: {err}"))?;

    let ctx = Context {
        overrides: &overrides,
        settings: &settings,
        method,
        exe: exe.as_deref(),
        work: &work,
        mode,
        now,
    };
    let result = inner(&ctx, &mut state);
    let _ = fs::remove_dir_all(&work);
    state.check_finished_at = Some(now_secs());
    match &result {
        Ok(_) => state.last_error = None,
        Err(message) => {
            state.last_error = Some(LastError {
                at: now_secs(),
                message: message.clone(),
            });
        }
    }
    let _ = state.save(home);
    result
}

struct Context<'a> {
    overrides: &'a Overrides,
    settings: &'a Settings,
    method: Method,
    exe: Option<&'a Path>,
    work: &'a Path,
    mode: Mode,
    now: i64,
}

fn remove_stale_work(update_dir: &Path, keep: &Path) {
    let Ok(entries) = fs::read_dir(update_dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path != keep && entry.file_name().to_string_lossy().starts_with("work-") {
            let _ = fs::remove_dir_all(path);
        }
    }
}

fn inner(ctx: &Context<'_>, state: &mut State) -> Result<Outcome, String> {
    let running = version::parse_version(&ctx.overrides.running_version())?;
    let pin = ctx
        .settings
        .pin
        .as_deref()
        .map(version::parse_version)
        .transpose()?;
    fs::create_dir_all(ctx.work)
        .map_err(|err| format!("cannot create {}: {err}", ctx.work.display()))?;

    let listing = ctx.work.join("releases.json");
    fetch::download(&listing_url(ctx.overrides), &listing, LISTING_SECS)?;
    let bytes = fs::read(&listing).map_err(|err| format!("cannot read the release list: {err}"))?;
    let releases = version::parse_listing(&bytes)?;
    let newest = version::newest_known(&releases, &running).map(|r| r.version.to_string());
    state.newest_known.clone_from(&newest);

    let rules = Rules {
        running: &running,
        pin: pin.as_ref(),
        manual: ctx.mode == Mode::Manual,
    };
    match version::pick(&releases, rules) {
        Pick::UpToDate => Ok(Outcome::UpToDate { newest }),
        Pick::PinMissing(pin) => Err(format!(
            "the pinned version {pin} is not a published release (`acs update unpin` clears it)"
        )),
        Pick::Notice(release) => Ok(notice(&release, NoticeKind::NewerMajor, &ctx.method)),
        Pick::Install(release) => {
            let replaceable = ctx.method == Method::Installer;
            let (Some(exe), true) = (ctx.exe, replaceable) else {
                return Ok(notice(&release, NoticeKind::NotReplaceable, &ctx.method));
            };
            apply(ctx, exe, &release)?;
            let update = LastUpdate {
                from: running.to_string(),
                to: release.version.to_string(),
                at: now_secs(),
            };
            state.last_update = Some(update.clone());
            Ok(Outcome::Updated {
                from: update.from,
                to: update.to,
            })
        }
    }
}

fn notice(release: &Release, kind: NoticeKind, method: &Method) -> Outcome {
    Outcome::Notice {
        version: release.version.to_string(),
        kind,
        command: method.manual_update_command(),
    }
}

/// Downloads, verifies, unpacks, tries and swaps in `release`.
fn apply(ctx: &Context<'_>, exe: &Path, release: &Release) -> Result<(), String> {
    let key = signature::release_key(ctx.overrides.public_key.as_deref())
        .map_err(|err| err.to_string())?;
    let target = target_triple().ok_or("this platform cannot replace itself")?;
    let archive_name = format!("acs-cli-{target}.zip");
    let archive = ctx.work.join(&archive_name);
    let sig_path = ctx.work.join(format!("{archive_name}.sig"));
    fetch::download(
        &asset_url(ctx.overrides, &release.tag, &archive_name),
        &archive,
        DOWNLOAD_SECS,
    )?;
    fetch::download(
        &asset_url(ctx.overrides, &release.tag, &format!("{archive_name}.sig")),
        &sig_path,
        fetch::LISTING_SECS,
    )?;
    let archive_bytes =
        fs::read(&archive).map_err(|err| format!("cannot read the download: {err}"))?;
    let signature_text =
        fs::read_to_string(&sig_path).map_err(|err| format!("cannot read the signature: {err}"))?;
    // Nothing is unpacked before this passes.
    signature::verify(
        &key,
        &release.tag,
        &archive_name,
        &archive_bytes,
        &signature_text,
    )
    .map_err(|err| format!("{err} ({})", release.tag))?;

    let unpacked: PathBuf = ctx.work.join("unpacked");
    fs::create_dir_all(&unpacked)
        .map_err(|err| format!("cannot create {}: {err}", unpacked.display()))?;
    fetch::unpack(&archive, &unpacked)?;
    let exe_name = exe
        .file_name()
        .map_or_else(|| "acs.exe".into(), |n| n.to_string_lossy().into_owned());
    let found = fetch::find_unpacked(&unpacked, &exe_name)
        .ok_or_else(|| format!("the archive holds no {exe_name}"))?;

    let placed = swap::place_new(exe, &found)?;
    if let Err(why) = swap::try_version(&placed, &release.version.to_string()) {
        let _ = fs::remove_file(&placed);
        return Err(why);
    }
    swap::swap(exe, &placed, ctx.now).map_err(|why| {
        let _ = fs::remove_file(&placed);
        why
    })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn addresses_are_github_unless_a_test_overrides_them() {
        let plain = Overrides::default();
        assert_eq!(
            listing_url(&plain),
            "https://api.github.com/repos/acs-hq/acs/releases?per_page=30"
        );
        assert_eq!(
            asset_url(&plain, "v1.0.0", "a.zip"),
            "https://github.com/acs-hq/acs/releases/download/v1.0.0/a.zip"
        );
        let test = Overrides {
            base_url: Some("http://127.0.0.1:5/".to_owned()),
            ..Overrides::default()
        };
        assert_eq!(listing_url(&test), "http://127.0.0.1:5/releases");
        assert_eq!(
            asset_url(&test, "v1.0.0", "a.zip"),
            "http://127.0.0.1:5/download/v1.0.0/a.zip"
        );
    }

    #[test]
    fn a_build_that_does_not_check_returns_before_touching_anything() {
        // Only meaningful where the feature and overrides are absent.
        if Overrides::current().build_enabled() {
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let home = AcsHome::at(tmp.path().join("home")).unwrap();
        spawn_if_due(Some(&home), 1_000_000);
        assert!(!home.root().exists());
        assert!(matches!(
            run(&home, Mode::Background, 1_000_000),
            Ok(Outcome::Off(_))
        ));
        assert!(!home.root().exists());
    }
}
