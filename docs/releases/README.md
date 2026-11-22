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

ACS is installed with the PowerShell and shell installers, or with Scoop. It is not published to
npm: an npm package puts a launcher in front of `acs`, and every hook call would pay for it
(decision 0053).

Run `dist generate` after changing `dist-workspace.toml`. Without it the `release plan` CI job fails.

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
