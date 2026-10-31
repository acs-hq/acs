# Releasing ACS

The release workflow runs on a version tag. It has never run for real, so expect the first one to
find something (decision 0021 lists what is unverified).

Before any public launch, announcement or public release: ACS has only been proven across Claude
Code sessions, not across harnesses, until the Antigravity gate passes (decision 0019).

## Set up once (human)

| What | Why |
| --- | --- |
| Finish the name check (npm, Homebrew, trademarks) | `acs-hq/homebrew-tap` and `@acs-hq/acs` in `dist-workspace.toml` are placeholders until then |
| Make `acs-hq/acs` public | Every installer, the npm package, the Scoop manifest and the setup skill's raw URL fetch public URLs |
| Create the repository `acs-hq/homebrew-tap` | dist commits the formula there |
| Add the secret `HOMEBREW_TAP_TOKEN` to `acs-hq/acs` | A token with write access to the tap |
| Create the npm scope `@acs-hq` and add the secret `NPM_TOKEN` | `publish-npm` uses it |

Run `dist generate` after changing `dist-workspace.toml`. Without it the `release plan` CI job fails.

## Cut a release

1. Make sure CI is green on `main`, and that the version in `Cargo.toml` and in
   `integrations/claude-code/.claude-plugin/plugin.json` is the one you are releasing.
2. Run the manual check of plan section 9.1 and record it here as `docs/releases/v<version>.md`.
3. Tag and push:

   ```sh
   git tag v0.1.0
   git push origin v0.1.0
   ```

4. Watch the `Release` workflow. After the announcement it calls `Installers`, which tries every
   channel on a clean machine.

If a publish job fails, the GitHub Release already exists and `announce` and `Installers` are
skipped. Fix the cause, then run `Installers` by hand with the tag.
