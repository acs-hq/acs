# Releasing ACS

The release workflow runs on a version tag. It has never run for real, so expect the first one to
find something (decision 0021 lists what is unverified).

Before any public launch, announcement or public release: ACS has only been proven across Claude
Code sessions, not across harnesses, until the Antigravity gate passes (decision 0019).

## Set up once (human)

| What | Why |
| --- | --- |
| Finish the name check (trademarks, package names) | The names `acs` and `acs-cli` are placeholders until then |
| Make `acs-hq/acs` public | Every installer, the Scoop manifest and the setup skill's raw URL fetch public URLs |
| Create the release signing key, store the private half as the secret `ACS_RELEASE_SIGNING_KEY` (a PEM) and put the public half in `RELEASE_PUBLIC_KEY` (`crates/acs-cli/src/update/signature.rs`); see "Signing" below | The release's `Sign release` job fails without the secret, and an `acs` built with the all-zero key refuses every update (decision 0054) |

ACS is installed with the PowerShell and shell installers, or with Scoop. It is not published to
npm: an npm package puts a launcher in front of `acs`, and every hook call would pay for it
(decision 0053).

Run `dist generate` after changing `dist-workspace.toml`. Without it the `release plan` CI job fails.

## Signing (the updater, decision 0054)

`acs` updates itself from the GitHub Release, so every archive must carry a signature. The
`Sign release` job (`.github/workflows/sign-release.yml`, listed in `global-artifacts-jobs`) writes
`<archive>.sig` next to each `acs-cli-*.zip` and `acs-cli-*.tar.xz`: the 64-byte Ed25519 signature
in base64 on one line, over the message `.github/sign-release-message.sh` prints (the tag, the file
name and the file's SHA-256). The job fails if the secret is missing, so a release is never
published unsigned.

Create the key once, and keep the private half off every machine a session can read:

```sh
openssl genpkey -algorithm ed25519 -out acs-release-key.pem
# the 32 bytes to paste into RELEASE_PUBLIC_KEY (the last 32 bytes of the public key's DER)
openssl pkey -in acs-release-key.pem -pubout -outform DER | tail -c 32 | od -An -tx1 -v
gh secret set ACS_RELEASE_SIGNING_KEY --repo acs-hq/acs < acs-release-key.pem
```

Then store the PEM somewhere safe and delete it from the working folder. Releasing from a build
whose `RELEASE_PUBLIC_KEY` does not match the secret produces releases that nobody can update to;
the Windows run below catches that.

The updater is tried with pre-release tags. `dist` only releases a tag that equals the version in
`Cargo.toml`, so each tag needs a commit that sets the version first (in `Cargo.toml` and
`integrations/antigravity/plugin.json`, then `cargo metadata` to refresh `Cargo.lock`): set
`1.0.0-rc.1`, tag `v1.0.0-rc.1`, install it with the PowerShell installer, set `1.0.0-rc.2`, tag
`v1.0.0-rc.2`, and see `acs` replace itself (`acs update` does it at once; the daily
check does it by itself). A release candidate follows release candidates, and the first stable
`v1.0.0` after them. Record that run as `docs/releases/v1.0.0-updater.md`. If your antivirus
blocks the new file, `acs doctor` shows it; add an exception only if the run shows one is needed.

## Cut a release

1. Make sure CI is green on `main`, and that the version in `Cargo.toml` and in
   `integrations/antigravity/plugin.json` is the one you are releasing. The Claude Code plugin
   pins no version, so Claude Code follows the commit (issue 58).
2. Run the manual check of plan section 9.1 and record it here as `docs/releases/v<version>.md`.
3. Tag and push:

   ```sh
   git tag v1.0.0
   git push origin v1.0.0
   ```

4. Watch the `Release` workflow. After the announcement it calls `Installers`, which tries every
   channel on a clean machine.

If a publish job fails, the GitHub Release already exists and `announce` and `Installers` are
skipped. Fix the cause, then run `Installers` by hand with the tag.

## Turn Homebrew on later

Each release already ships the formula as the file `acs.rb`; it just is not published. To publish it:

1. Create the repository `acs-hq/homebrew-tap` and add the secret `HOMEBREW_TAP_TOKEN` to
   `acs-hq/acs`, a token with write access to the tap.
2. Set `publish-jobs = ["homebrew"]` in `dist-workspace.toml` and run `dist generate`.
3. Add a job to `.github/workflows/installers.yml` that runs `brew install acs-hq/tap/acs` and
   `sh .github/smoke.sh "$TAG"` on `macos-latest`, and add that `brew install` line to the README.
