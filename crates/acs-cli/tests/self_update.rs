//! The self-updater end to end (decision 0054): the built `acs.exe` is copied into a temporary
//! install directory laid out like the PowerShell installer's `cargo-home`, a receipt is written
//! to a temporary `%LOCALAPPDATA%`, and a tiny HTTP server on 127.0.0.1 plays GitHub. The test
//! overrides (`ACS_UPDATE_TEST_*`) exist only in debug builds, which is what `cargo test` builds.
//!
//! The archive served is the freshly built `acs.exe` itself, which prints the real crate version,
//! so the release the server announces is `v<CARGO_PKG_VERSION>` while the running copy pretends
//! to be older.

#![cfg(windows)]
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use acs_cli::update::check::target_triple;
use acs_cli::update::fetch::tar_path;
use acs_cli::update::signature::{b64_encode, message, sha256_hex};
use common::{World, acs_bin};
use ed25519_compact::{KeyPair, Seed};
use serde_json::{Value, json};
use tempfile::TempDir;

const VERSION: &str = env!("CARGO_PKG_VERSION");

fn tag() -> String {
    format!("v{VERSION}")
}

fn archive_name() -> String {
    format!("acs-cli-{}.zip", target_triple().unwrap())
}

// ---------------------------------------------------------------------------------------------
// The fake GitHub
// ---------------------------------------------------------------------------------------------

struct Server {
    port: u16,
    hits: Arc<Mutex<Vec<String>>>,
}

impl Server {
    fn url(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }

    fn hits(&self) -> Vec<String> {
        self.hits.lock().unwrap().clone()
    }
}

/// Serves `routes` (path to body) and records every request path. The thread ends with the test
/// process.
fn serve(routes: Vec<(String, Vec<u8>)>) -> Server {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let hits = Arc::new(Mutex::new(Vec::new()));
    let seen = Arc::clone(&hits);
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut first = String::new();
            if reader.read_line(&mut first).is_err() {
                continue;
            }
            loop {
                let mut line = String::new();
                match reader.read_line(&mut line) {
                    Ok(0) | Err(_) => break,
                    Ok(_) if line.trim().is_empty() => break,
                    Ok(_) => {}
                }
            }
            let path = first.split_whitespace().nth(1).unwrap_or("/").to_owned();
            seen.lock().unwrap().push(path.clone());
            let body = routes.iter().find(|(p, _)| *p == path).map(|(_, b)| b);
            let head = match body {
                Some(body) => format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                ),
                None => "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                    .to_owned(),
            };
            let _ = stream.write_all(head.as_bytes());
            if let Some(body) = body {
                // In small pieces: one huge `write` on Windows can be "accepted" at once and
                // then cut off by the close that follows, resetting the client mid-body.
                for piece in body.chunks(32 * 1024) {
                    if stream.write_all(piece).is_err() {
                        break;
                    }
                }
            }
            // Close gracefully: end our side, then wait for the client to finish reading, so that
            // the end of a large body is never lost to a reset.
            let _ = stream.flush();
            let _ = stream.shutdown(std::net::Shutdown::Write);
            // A scanner on the machine can stall curl's writes for tens of seconds; closing
            // before it has read everything would reset the connection under it.
            let _ = stream.set_read_timeout(Some(Duration::from_secs(120)));
            let _ = std::io::copy(&mut reader, &mut std::io::sink());
        }
    });
    Server { port, hits }
}

/// An address nothing listens on.
fn offline_url() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    format!("http://127.0.0.1:{port}")
}

// ---------------------------------------------------------------------------------------------
// Signing and archives
// ---------------------------------------------------------------------------------------------

fn key(seed: u8) -> KeyPair {
    KeyPair::from_seed(Seed::new([seed; 32]))
}

fn public_hex(seed: u8) -> String {
    key(seed).pk.iter().map(|b| format!("{b:02x}")).collect()
}

fn signature_file(seed: u8, tag: &str, name: &str, archive: &[u8]) -> Vec<u8> {
    let text = message(tag, name, &sha256_hex(archive));
    let sig = key(seed).sk.sign(text.as_bytes(), None);
    format!("{}\n", b64_encode(sig.as_ref())).into_bytes()
}

/// A zip holding the built `acs.exe` at its root, made with the same `tar.exe` the updater uses.
fn zip_of_built_binary(dir: &Path) -> Vec<u8> {
    let staging = dir.join("zip-staging");
    fs::create_dir_all(&staging).unwrap();
    fs::copy(acs_bin(), staging.join("acs.exe")).unwrap();
    let zip = dir.join("release.zip");
    let status = Command::new(tar_path())
        .arg("-a")
        .arg("-cf")
        .arg(&zip)
        .arg("-C")
        .arg(&staging)
        .arg("acs.exe")
        .status()
        .unwrap();
    assert!(status.success(), "tar could not make the zip");
    fs::read(zip).unwrap()
}

fn releases_json(tags: &[&str]) -> Vec<u8> {
    let list: Vec<Value> = tags
        .iter()
        .map(|t| json!({"tag_name": t, "draft": false, "prerelease": t.contains('-')}))
        .collect();
    serde_json::to_vec(&list).unwrap()
}

/// The routes of a healthy release `tag`, signed with `seed`.
fn release_routes(dir: &Path, tag: &str, sign_with: u8) -> Vec<(String, Vec<u8>)> {
    let zip = zip_of_built_binary(dir);
    let name = archive_name();
    let sig = signature_file(sign_with, tag, &name, &zip);
    vec![
        ("/releases".to_owned(), releases_json(&[tag])),
        (format!("/download/{tag}/{name}"), zip),
        (format!("/download/{tag}/{name}.sig"), sig),
    ]
}

// ---------------------------------------------------------------------------------------------
// The install directory
// ---------------------------------------------------------------------------------------------

/// What marks the installed copy as the old one: bytes after the program, which Windows ignores.
const OLD_MARKER: &[u8] = b"\0old-install-marker";

struct Rig {
    tmp: TempDir,
    exe: PathBuf,
    local: PathBuf,
    home: PathBuf,
    old_bytes: Vec<u8>,
}

impl Rig {
    /// An install laid out like the installer's `cargo-home`, with a receipt that names it.
    fn installer() -> Self {
        let rig = Self::bare(&["cargo", "bin"]);
        let prefix = rig.exe.parent().unwrap().parent().unwrap();
        let receipt = json!({
            "binaries": ["acs"],
            "install_layout": "cargo-home",
            "install_prefix": prefix.to_string_lossy(),
            "modify_path": true,
            "source": {"app_name": "acs-cli", "name": "acs", "owner": "acs-hq", "release_type": "github"},
            "version": "0.9.0",
        });
        let folder = rig.local.join("acs-cli");
        fs::create_dir_all(&folder).unwrap();
        fs::write(
            folder.join("acs-cli-receipt.json"),
            format!("{receipt}\r\n"),
        )
        .unwrap();
        rig
    }

    /// An install under `scoop\apps\`, with no receipt.
    fn scoop() -> Self {
        Self::bare(&["scoop", "apps", "acs", "current"])
    }

    fn bare(dirs: &[&str]) -> Self {
        // Under the target directory: an antivirus may look harder at fresh programs in `%TEMP%`.
        let tmp = tempfile::Builder::new()
            .prefix("acs-update-")
            .tempdir_in(env!("CARGO_TARGET_TMPDIR"))
            .unwrap();
        let base = dunce::canonicalize(tmp.path()).unwrap();
        let mut dir = base.clone();
        for part in dirs {
            dir.push(part);
        }
        fs::create_dir_all(&dir).unwrap();
        let mut old_bytes = fs::read(acs_bin()).unwrap();
        old_bytes.extend_from_slice(OLD_MARKER);
        let exe = dir.join("acs.exe");
        fs::write(&exe, &old_bytes).unwrap();
        let local = base.join("local");
        fs::create_dir_all(&local).unwrap();
        Self {
            exe,
            local,
            home: base.join("acs-home"),
            old_bytes,
            tmp,
        }
    }

    fn dir(&self) -> &Path {
        self.exe.parent().unwrap()
    }

    fn work(&self) -> PathBuf {
        let path = self.tmp.path().join("work");
        fs::create_dir_all(&path).unwrap();
        path
    }

    /// The installed `acs.exe` with the updater pointed at `url`, trusting key `seed`, pretending
    /// to be version `running`.
    fn run(&self, args: &[&str], url: &str, seed: u8, running: &str) -> Output {
        Command::new(&self.exe)
            .args(args)
            .current_dir(self.tmp.path())
            .env("ACS_HOME", &self.home)
            .env("LOCALAPPDATA", &self.local)
            .env_remove("XDG_CONFIG_HOME")
            .env("ACS_UPDATE_TEST_BASE_URL", url)
            .env("ACS_UPDATE_TEST_PUBLIC_KEY", public_hex(seed))
            .env("ACS_UPDATE_TEST_RELEASE_BUILD", "1")
            .env("ACS_UPDATE_TEST_RUNNING_VERSION", running)
            .stdin(Stdio::null())
            .output()
            .unwrap()
    }

    fn state(&self) -> Option<Value> {
        let bytes = fs::read(self.home.join("update").join("state.json")).ok()?;
        serde_json::from_slice(&bytes).ok()
    }

    fn installed(&self) -> Vec<u8> {
        fs::read(&self.exe).unwrap()
    }

    fn leftovers(&self) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(self.dir())
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n != "acs.exe")
            .collect();
        names.sort();
        names
    }
}

fn json_of(output: &Output) -> Value {
    serde_json::from_slice(&output.stdout)
        .unwrap_or_else(|err| panic!("not JSON ({err}): {output:?}"))
}

fn built() -> Vec<u8> {
    fs::read(acs_bin()).unwrap()
}

// ---------------------------------------------------------------------------------------------
// Cases
// ---------------------------------------------------------------------------------------------

#[test]
fn an_old_build_updates_itself_and_records_it() {
    let rig = Rig::installer();
    let server = serve(release_routes(&rig.work(), &tag(), 1));
    let out = rig.run(&["update", "--json"], &server.url(), 1, "0.9.0");
    assert!(out.status.success(), "{out:?}");
    let report = json_of(&out);
    assert_eq!(report["result"], "updated", "{report}");
    assert_eq!(report["details"]["from"], "0.9.0");
    assert_eq!(report["details"]["to"], VERSION);

    assert_eq!(
        rig.installed(),
        built(),
        "the install now holds the release"
    );
    let leftovers = rig.leftovers();
    assert_eq!(leftovers.len(), 1, "{leftovers:?}");
    assert!(leftovers[0].starts_with("acs.exe.old-"), "{leftovers:?}");
    assert_eq!(
        fs::read(rig.dir().join(&leftovers[0])).unwrap(),
        rig.old_bytes
    );

    let state = rig.state().unwrap();
    assert_eq!(state["last_update"]["from"], "0.9.0");
    assert_eq!(state["last_update"]["to"], VERSION);
    assert_eq!(state["newest_known"], VERSION);
    assert!(state.get("last_error").is_none(), "{state}");

    // The new file runs.
    let version = Command::new(&rig.exe).arg("--version").output().unwrap();
    assert!(String::from_utf8_lossy(&version.stdout).contains(VERSION));

    // The next check removes what the update left, and finds nothing newer.
    let out = rig.run(&["update", "--json"], &server.url(), 1, VERSION);
    assert!(out.status.success(), "{out:?}");
    assert_eq!(json_of(&out)["result"], "up_to_date");
    assert!(rig.leftovers().is_empty(), "{:?}", rig.leftovers());
}

#[test]
fn the_background_check_updates_within_the_major_version() {
    let rig = Rig::installer();
    let server = serve(release_routes(&rig.work(), &tag(), 1));
    let major: u64 = env!("CARGO_PKG_VERSION_MAJOR").parse().unwrap();
    let running = format!("{major}.0.0-rc.0");
    let out = rig.run(&["update", "--background"], &server.url(), 1, &running);
    assert!(out.status.success(), "{out:?}");
    assert!(out.stdout.is_empty() && out.stderr.is_empty(), "{out:?}");
    assert_eq!(rig.installed(), built());
    assert_eq!(rig.state().unwrap()["last_update"]["to"], VERSION);
}

#[test]
fn the_background_check_only_notices_a_newer_major_version() {
    let rig = Rig::installer();
    let server = serve(release_routes(&rig.work(), &tag(), 1));
    let out = rig.run(&["update", "--background"], &server.url(), 1, "0.9.0");
    assert!(out.status.success(), "{out:?}");
    assert_eq!(rig.installed(), rig.old_bytes);
    let state = rig.state().unwrap();
    assert_eq!(state["newest_known"], VERSION);
    assert!(state.get("last_update").is_none(), "{state}");
}

#[test]
fn a_bad_signature_is_refused_and_doctor_shows_it() {
    let rig = Rig::installer();
    // Signed by a different key than the one the build trusts.
    let server = serve(release_routes(&rig.work(), &tag(), 2));
    let out = rig.run(&["update", "--json"], &server.url(), 1, "0.9.0");
    assert!(!out.status.success(), "{out:?}");
    let report = json_of(&out);
    assert_eq!(report["ok"], false);
    let message = report["error"]["message"].as_str().unwrap();
    assert!(message.contains("signature does not match"), "{message}");

    assert_eq!(rig.installed(), rig.old_bytes, "nothing changed");
    assert!(rig.leftovers().is_empty(), "{:?}", rig.leftovers());
    let state = rig.state().unwrap();
    assert!(
        state["last_error"]["message"]
            .as_str()
            .unwrap()
            .contains("signature does not match"),
        "{state}"
    );

    let doctor = rig.run(&["doctor", "--json"], &server.url(), 1, "0.9.0");
    let report = json_of(&doctor);
    let check = report["checks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["id"] == "update")
        .unwrap_or_else(|| panic!("no update check: {report}"));
    assert_eq!(check["status"], "warn", "{check}");
    assert!(
        check["message"]
            .as_str()
            .unwrap()
            .contains("signature does not match"),
        "{check}"
    );
}

#[test]
fn a_tampered_archive_is_refused() {
    let rig = Rig::installer();
    let mut routes = release_routes(&rig.work(), &tag(), 1);
    let zip_route = routes
        .iter_mut()
        .find(|(p, _)| p.ends_with(".zip"))
        .unwrap();
    zip_route.1.push(0);
    let server = serve(routes);
    let out = rig.run(&["update", "--json"], &server.url(), 1, "0.9.0");
    assert!(!out.status.success(), "{out:?}");
    assert_eq!(rig.installed(), rig.old_bytes);
    assert!(rig.leftovers().is_empty());
}

#[test]
fn a_signature_for_another_release_tag_is_refused() {
    let rig = Rig::installer();
    let zip = zip_of_built_binary(&rig.work());
    let name = archive_name();
    let tag = tag();
    // Signed for v0.0.1, served under the newer tag.
    let sig = signature_file(1, "v0.0.1", &name, &zip);
    let server = serve(vec![
        ("/releases".to_owned(), releases_json(&[&tag])),
        (format!("/download/{tag}/{name}"), zip),
        (format!("/download/{tag}/{name}.sig"), sig),
    ]);
    let out = rig.run(&["update", "--json"], &server.url(), 1, "0.9.0");
    assert!(!out.status.success(), "{out:?}");
    assert_eq!(rig.installed(), rig.old_bytes);
}

#[test]
fn the_built_in_release_key_refuses_an_archive_signed_by_another_key() {
    let rig = Rig::installer();
    let server = serve(release_routes(&rig.work(), &tag(), 1));
    // No key override: the build checks with the real release key, and the stand-in signs with a
    // test key, so the archive is refused and the installed file is untouched.
    let out = Command::new(&rig.exe)
        .args(["update", "--json"])
        .current_dir(rig.tmp.path())
        .env("ACS_HOME", &rig.home)
        .env("LOCALAPPDATA", &rig.local)
        .env_remove("XDG_CONFIG_HOME")
        .env_remove("ACS_UPDATE_TEST_PUBLIC_KEY")
        .env("ACS_UPDATE_TEST_BASE_URL", server.url())
        .env("ACS_UPDATE_TEST_RELEASE_BUILD", "1")
        .env("ACS_UPDATE_TEST_RUNNING_VERSION", "0.9.0")
        .output()
        .unwrap();
    assert!(!out.status.success(), "{out:?}");
    let report = json_of(&out);
    assert!(
        report["error"]["message"]
            .as_str()
            .unwrap()
            .contains("signature does not match"),
        "{report}"
    );
    assert_eq!(rig.installed(), rig.old_bytes);
}

#[test]
fn offline_changes_nothing_and_records_the_failure() {
    let rig = Rig::installer();
    let out = rig.run(&["update", "--json"], &offline_url(), 1, "0.9.0");
    assert!(!out.status.success(), "{out:?}");
    assert_eq!(rig.installed(), rig.old_bytes);
    assert!(rig.leftovers().is_empty());
    let state = rig.state().unwrap();
    assert!(
        state["last_error"]["message"]
            .as_str()
            .unwrap()
            .contains("failed"),
        "{state}"
    );
    assert!(state["check_started_at"].is_i64());
    assert!(state["check_finished_at"].is_i64());
}

#[test]
fn auto_off_makes_no_request_from_the_background_path() {
    let rig = Rig::installer();
    let server = serve(release_routes(&rig.work(), &tag(), 1));
    let out = rig.run(&["update", "off"], &server.url(), 1, "0.9.0");
    assert!(out.status.success(), "{out:?}");
    let settings = fs::read_to_string(rig.home.join("update").join("settings.toml")).unwrap();
    assert!(settings.contains("auto = false"), "{settings}");

    let out = rig.run(&["update", "--background"], &server.url(), 1, "0.9.0");
    assert!(out.status.success(), "{out:?}");
    assert!(server.hits().is_empty(), "{:?}", server.hits());
    assert!(rig.state().is_none());
    assert_eq!(rig.installed(), rig.old_bytes);

    // Doctor says so, without any request.
    let doctor = rig.run(&["doctor", "--json"], &server.url(), 1, "0.9.0");
    let report = json_of(&doctor);
    let message = report["checks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["id"] == "update")
        .unwrap()["message"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(message.contains("Updating is off"), "{message}");
    assert!(server.hits().is_empty());
}

#[test]
fn a_pin_to_an_older_version_is_exactly_that_version() {
    let rig = Rig::installer();
    let server = serve(release_routes(&rig.work(), &tag(), 1));
    let pin = rig.run(&["update", "pin", VERSION], &server.url(), 1, "0.9.0");
    assert!(pin.status.success(), "{pin:?}");
    // Running a build that is already past the pin, the check goes back to it.
    let out = rig.run(&["update", "--json"], &server.url(), 1, "99.0.0");
    assert!(out.status.success(), "{out:?}");
    assert_eq!(json_of(&out)["result"], "updated");
    assert_eq!(rig.installed(), built());
    let unpin = rig.run(&["update", "unpin"], &server.url(), 1, VERSION);
    assert!(unpin.status.success(), "{unpin:?}");
}

#[test]
fn a_scoop_install_gets_a_notice_and_no_change() {
    let rig = Rig::scoop();
    let server = serve(release_routes(&rig.work(), &tag(), 1));
    let out = rig.run(&["update", "--json"], &server.url(), 1, "0.9.0");
    assert!(out.status.success(), "{out:?}");
    let report = json_of(&out);
    assert_eq!(report["result"], "notice", "{report}");
    assert_eq!(report["details"]["command"], "scoop update acs");
    assert_eq!(rig.installed(), rig.old_bytes);
    assert!(rig.leftovers().is_empty());
    // Only the listing was fetched.
    assert_eq!(server.hits(), ["/releases"]);
    assert_eq!(rig.state().unwrap()["newest_known"], VERSION);
}

#[test]
fn a_release_no_newer_than_the_running_version_changes_nothing() {
    let rig = Rig::installer();
    let server = serve(release_routes(&rig.work(), &tag(), 1));
    let out = rig.run(&["update", "--json"], &server.url(), 1, VERSION);
    assert!(out.status.success(), "{out:?}");
    assert_eq!(json_of(&out)["result"], "up_to_date");
    assert_eq!(rig.installed(), rig.old_bytes);
    assert_eq!(server.hits(), ["/releases"]);

    // An older running version is never replaced by an older release either.
    let server = serve(vec![(
        "/releases".to_owned(),
        releases_json(&["v0.5.0", "v0.4.0"]),
    )]);
    let out = rig.run(&["update", "--json"], &server.url(), 1, "0.9.0");
    assert!(out.status.success(), "{out:?}");
    assert_eq!(json_of(&out)["result"], "up_to_date");
    assert_eq!(rig.installed(), rig.old_bytes);
}

#[test]
fn a_second_background_check_is_not_started_within_a_day() {
    let rig = Rig::installer();
    let server = serve(release_routes(&rig.work(), &tag(), 1));
    let out = rig.run(&["update", "--background"], &server.url(), 1, "0.9.0");
    assert!(out.status.success(), "{out:?}");
    let hits = server.hits().len();
    assert!(hits >= 1);
    // Started just now: not due again.
    let out = rig.run(&["update", "--background"], &server.url(), 1, "0.9.0");
    assert!(out.status.success(), "{out:?}");
    assert_eq!(server.hits().len(), hits);
}

// ---------------------------------------------------------------------------------------------
// The session-start hook
// ---------------------------------------------------------------------------------------------

fn session_start(world: &World, envs: &[(&str, String)]) {
    let event = json!({
        "spec": "acs/0.1",
        "harness": "claude-code",
        "harness_session_id": "updater-test",
        "phase": "session_start",
        "cwd": world.repo,
    });
    let mut command = Command::new(acs_bin());
    command
        .args(["hook", "--canonical", "session_start"])
        .current_dir(&world.repo)
        .env("ACS_HOME", &world.home)
        .env("GIT_CONFIG_GLOBAL", &world.git_config)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env_remove("ACS_UPDATE_TEST_BASE_URL")
        .env_remove("ACS_UPDATE_TEST_RELEASE_BUILD")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (name, value) in envs {
        command.env(name, value);
    }
    let mut child = command.spawn().unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(event.to_string().as_bytes())
        .unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(out.status.success(), "{out:?}");
    let text = String::from_utf8_lossy(&out.stdout).to_lowercase();
    assert!(
        !text.contains("update"),
        "the hook said something about updates: {text}"
    );
}

#[test]
fn session_start_spawns_nothing_for_a_build_that_does_not_update_itself() {
    let world = World::new();
    session_start(&world, &[]);
    std::thread::sleep(Duration::from_millis(1500));
    assert!(
        !world.home.join("update").exists(),
        "a build without the feature touched the update folder"
    );
}

#[test]
fn session_start_starts_a_background_check_when_one_is_due_and_not_when_off() {
    let world = World::new();
    let envs = [
        ("ACS_UPDATE_TEST_RELEASE_BUILD", "1".to_owned()),
        ("ACS_UPDATE_TEST_BASE_URL", offline_url()),
    ];
    // auto = false: nothing starts.
    fs::create_dir_all(world.home.join("update")).unwrap();
    fs::write(
        world.home.join("update").join("settings.toml"),
        "auto = false\n",
    )
    .unwrap();
    session_start(&world, &envs);
    std::thread::sleep(Duration::from_millis(1500));
    assert!(!world.home.join("update").join("state.json").exists());

    // auto on: the detached check starts and records its (offline) failure.
    fs::write(
        world.home.join("update").join("settings.toml"),
        "auto = true\n",
    )
    .unwrap();
    session_start(&world, &envs);
    let state = world.home.join("update").join("state.json");
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        if let Ok(bytes) = fs::read(&state)
            && let Ok(value) = serde_json::from_slice::<Value>(&bytes)
            && value.get("last_error").is_some()
        {
            break;
        }
        assert!(Instant::now() < deadline, "no background check ran");
        std::thread::sleep(Duration::from_millis(200));
    }
}
