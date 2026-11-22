# Updates

A release build of `acs` checks for a newer release once a day. On a Windows install made with
the PowerShell installer it then replaces itself. Everywhere else it only tells you that a new
version exists.

## Who updates itself

| How you installed `acs` | What happens |
| --- | --- |
| The PowerShell installer, on Windows | `acs` downloads the new version, checks its signature and replaces its own file. |
| Scoop | A notice only. Update with `scoop update acs`. |
| macOS and Linux | A notice only. Run the installer line from the README again. |
| `cargo install` or any build you made yourself | Nothing. Only release builds check for updates. |

Automatic updates stay inside the major version you run: `1.0.3` becomes `1.1.0`, never `2.0.0`.
A newer major version is a notice, and `acs update` installs it when you ask. A release candidate
(`1.0.0-rc.1`) follows release candidates; a stable build ignores them.

## What it contacts

Once a day, `acs` asks `api.github.com` for the list of releases of `acs-hq/acs`. If a newer one
is due, it downloads two files from `github.com`: the archive and its signature. That is the only
contact with any server besides your own git remote. Nothing about you or your repositories is
sent.

The check runs in a separate background process that the first session of the day starts. It
never runs inside a hook, never slows a tool call and never prints anything into an agent's
session. If you are offline, it fails quietly and tries again the next day.

## Why you can trust a download

Every release archive is signed by the project's key, and `acs` carries the matching public key.
The signature covers the version, the file name and the file's contents, so a tampered file, or an
old release passed off as a new one, is refused before anything is unpacked. Before the swap the
new program has to start and report the version it was promised to be.

If anything goes wrong (no network, a bad signature, a program that will not start), the running
`acs` stays as it is and `acs doctor` shows the reason.

## The commands

| Command | What it does |
| --- | --- |
| `acs update` | Check now and install a newer release, including a newer major version. |
| `acs update off` | Never contact GitHub in the background. |
| `acs update on` | Check every day again (the default). |
| `acs update pin 1.0.1` | Stay on exactly that version, and nothing after it. Also moves back to it. |
| `acs update unpin` | Follow new releases again. |

These are for people. An agent's `acs update ...` is refused, and so is editing the settings.
`acs doctor` has an `Updates` line: the version, how it was installed, whether updating is on, off
or pinned, the newest version seen and when, and the last failure.

To keep a machine from ever updating, install with `ACS_CLI_DISABLE_UPDATE=1` set (the installer
then writes no receipt, and `acs` treats the install as notice only), or run `acs update off`.

## Good to know

- The old `acs.exe` is renamed to `acs.exe.old-<time>` and deleted at a later check. A session
  that is already running keeps using it until it ends.
- A hook that starts in the instant between the two renames finds no `acs`. Claude Code lets that
  one call through; Antigravity refuses it.
- If your antivirus blocks the new file, the update fails and `acs doctor` says so. Add an
  exception for the install folder, or update by hand with the installer line.
