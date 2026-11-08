# WP7.3 and WP7.4: people in the loop (VS Code extension, `acs watch`)

Built 2026-11-09 (decision 0028). This page records the automated evidence and the in-editor
checks only the human can run.

## Automated evidence

- `cargo test -p acs-cli --test watch` (5 tests): a file changed outside any agent is claimed for
  the person and an agent's edit is then denied, naming `sara@example.com (human, ...)`; an agent's
  file-tool edit is left to the agent; a change made while an agent's shell command runs is held
  back until its post-tool step attributes it (the test fails when the hold-back is switched off);
  gitignored files are never claimed; a claim is released after the idle window.
- `npm test` in `integrations/vscode` (13 tests): the claim timing (claim after the first edit,
  refresh, release when saved and idle or closed, retry after a refusal), the display helpers, and
  the extension's CLI client against the real `acs` binary: a claim taken through it denies an
  agent's edit with the person named, and a file an agent holds is refused with the agent named.
- The compiled extension activates against a stubbed `vscode` module and reads this repository's
  `acs status` (a throwaway check, not committed).

## In-editor checks (the human's, before publishing)

Use a throwaway repository with `acs init`, or this one. Install the build from step 1, then:

1. Build and install: in `integrations/vscode`, `npm install`, `npm run package`, then
   `code --install-extension acs-vscode.vsix`. (Or open `integrations/vscode` in VS Code and press
   F5 to run it in an Extension Development Host without installing.)
2. Open the repository. The status bar shows `ACS $(lock) N`; clicking it lists claims.
3. Type in a tracked file, without saving. Within about a second `acs status` in a terminal lists
   it under your claims, and the status bar shows `$(person) 1`.
4. Ask a Claude Code session in the same repository to edit that file. It is denied, and the
   message names you (`... (human, active ...)`).
5. Save the file and leave it for two minutes (`acs.releaseAfterIdleSeconds`). The claim is gone
   from `acs status`, and the agent can now edit it.
6. While the agent holds a file (ask it to edit one and keep going), check that the file shows a
   lock badge in the explorer, a lock in the gutter on line 1, and a hover naming the agent.
7. Type in the file the agent holds. A warning says ACS could not claim it and names the agent.
8. Let the agent change a file you have open but have not edited: it must not appear under your
   claims.
9. Optional: in another editor, run `acs watch` in a terminal and repeat steps 3 to 5 there.

| Step | Result | Notes |
| --- | --- | --- |
| 1 | | |
| 2 | | |
| 3 | | |
| 4 | | |
| 5 | | |
| 6 | | |
| 7 | | |
| 8 | | |
| 9 | | |
