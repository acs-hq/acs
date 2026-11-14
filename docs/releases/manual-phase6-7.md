# Manual test: lore (Phase 6) and territories and people (Phase 7)

A script for trying Phases 6 and 7 by hand in a real repository with a git remote, using
several Claude Code sessions. Antigravity is not used. Fill in the results table at the end as you
go.

## Placeholders

Pick folders in your repository that hold code and substitute them everywhere below:

| Placeholder | Role in the tests | Pick |
| --- | --- | --- |
| `src/billing/` | Owned by a made-up teammate, `lina@example.com`; your agents are denied | Any code folder |
| `src/core/` | Owned by you; your agents may edit it | Another code folder |
| `src/api/` | Has lore | A third folder, ideally with several files |
| `src/legacy/` | Has lore written long ago, so it is out of date | A folder that changed a lot over the repository's history |

`ME` is your `git config user.email` in that repository. "Session A" and "Session B" are two
Claude Code sessions open in the repository at the same time, each started from its own
terminal.

**Seeing what ACS told an agent.** Hook messages are not printed in the chat. Ask the agent to
quote them ("quote any message from ACS word for word"), or press Ctrl+O in Claude Code to open
the transcript, which shows them as `PreToolUse:... hook additional context`.

**One instruction for every agent prompt below:** add "If ACS blocks something, quote its message
word for word and do not try to get around it." Otherwise an agent may try another route, which
is a test of its own (T8), not of the feature.

## 0. Clean out the old ACS and start fresh

**Since issue 63, `acs uninstall --all` does steps 3 to 7 in one command** (decision 0039). The
steps below still work and show what it does.

Close every Claude Code session in the repository first. Run everything in Git Bash, from the
repository root, on `main`.

1. **Check the new build is the one on `PATH`.** `acs lore --help` must work. If it says there is
   no `lore` command, the reinstall did not take.

   ```bash
   acs --version
   ```

   ```bash
   acs lore --help
   ```

2. **Note the repository id.** You need it in step 7. Save the `repo_id` value.

   ```bash
   acs status --json
   ```

3. **Remove ACS's files from the repository.** `.acs/` holds the old policy. In
   `.claude/settings.json`, remove the `acs-plugins` entry under `extraKnownMarketplaces` and
   `acs@acs-plugins` under `enabledPlugins`; delete the file if nothing else is left in it. If an
   old Antigravity setup exists, remove `.agents/plugins/acs/` too.

   ```bash
   git rm -r -q .acs
   ```

4. **Remove the old git hooks.** Look at each of `.git/hooks/pre-commit`,
   `.git/hooks/prepare-commit-msg` and `.git/hooks/pre-push`. If a file holds only the block
   between `# >>> acs git-hook ... >>>` and `# <<< acs git-hook ... <<<`, delete it. If it has
   other content too, delete just that block. If a file named `<hook>.acs-orig` exists next to it,
   rename it back to `<hook>`: that was your own hook before ACS wrapped it.

   ```bash
   ls -la .git/hooks
   ```

5. **Remove the local claim refs and the fetch setting.** The first command shows the fetch
   settings. Remove the ACS one (`+refs/acs/*:refs/remotes/origin/acs/*`) with the second, then
   delete the refs with the third.

   ```bash
   git config --get-all remote.origin.fetch
   ```

   ```bash
   git config --unset remote.origin.fetch '^\+refs/acs/'
   ```

   ```bash
   git for-each-ref --format='%(refname)' refs/acs refs/remotes/origin/acs | xargs -r -n1 git update-ref -d
   ```

6. **Remove the claim and ledger refs on the remote.** List them, then delete each one. Also
   delete an old `acs/adopt` branch if you pushed one.

   ```bash
   git ls-remote origin 'refs/acs/*'
   ```

   ```bash
   git ls-remote origin 'refs/acs/*' | cut -f2 | xargs -r -n1 git push origin --delete
   ```

7. **Delete the old local state.** It lives outside the repository, under your user folder.
   Replace `<repo_id>` with the value from step 2.

   ```bash
   rm -rf ~/.acs/repos/<repo_id>
   ```

8. **Refresh the Claude Code plugin.** The plugin's `plugin.json` still says version 0.1.0, so
   Claude Code keeps an old cached copy of its skills ([acs-hq/acs#58](https://github.com/acs-hq/acs/issues/58)).
   None of the tests below depends on the skill text, because hooks and the MCP tools run the
   `acs` you just installed. Run these to get the current skill anyway:

   ```bash
   claude plugin uninstall acs@acs-plugins
   ```

   ```bash
   rm -rf ~/.claude/plugins/cache/acs-plugins/acs
   ```

   ```bash
   claude plugin marketplace update acs-plugins
   ```

9. **Commit the removal.**

   ```bash
   git commit -q -am "Remove the old ACS setup"
   ```

### Fresh setup

1. **Set ACS up.** `acs init` offers zones from your history. Answer either way: you replace them
   in the next step.

   ```bash
   acs init
   ```

   ```bash
   acs plugin install claude-code
   ```

2. **Write the test policy.** Replace `.acs/policy/territory.toml` with this, using your folders
   and your email for `ME`:

   ```toml
   [[zone]]
   paths = ["src/billing/**"]
   kind = "owned"
   owners = ["lina@example.com"]

   [[zone]]
   paths = ["src/core/**"]
   kind = "owned"
   owners = ["ME"]
   ```

3. **Write three lore entries.** First get two commit ids. Then write the files below, putting
   the first id in place of `HEAD_ID` and the second in place of `OLD_ID`. Write the comment
   marker your language uses (`//` or `#`) in place of `//`.

   ```bash
   git rev-parse --short=12 HEAD
   ```

   ```bash
   git log --reverse --format=%h -- src/legacy | head -n 1
   ```

   `.acs/policy/lore/src/api.md`:

   ```markdown
   +++
   id = "lore_api"
   scope = ["src/api/**"]
   owner = "ME"
   written_against = "HEAD_ID"
   +++
   Every function you add or change in src/api/ gets a one-line comment directly above it that
   starts with `// API:` and says who calls it.
   ```

   `.acs/policy/lore/src.md`:

   ```markdown
   +++
   id = "lore_src"
   scope = ["src/**"]
   owner = "ME"
   written_against = "HEAD_ID"
   +++
   When you finish a change under src/, end your final message to the user with the line
   "Lore checked."
   ```

   `.acs/policy/lore/src/legacy.md`:

   ```markdown
   +++
   id = "lore_legacy"
   scope = ["src/legacy/**"]
   owner = "ME"
   written_against = "OLD_ID"
   +++
   src/legacy/ is frozen: only fix bugs here, never add features.
   ```

4. **Check, commit and push.** `acs doctor` should report `0 failure(s)`, `acs lore list` should
   show three entries and no problems, and `acs lore stale` should list `lore_legacy`.

   ```bash
   acs doctor
   ```

   ```bash
   acs lore list
   ```

   ```bash
   acs lore stale
   ```

   ```bash
   git add .acs .claude && git commit -q -m "Adopt ACS with test zones and lore" && git push
   ```

5. Start Claude Code in the repository and accept the plugin and the folder trust prompt.

## Phase 7: territories, overrides, self-protection, people

### T1. An owned zone denies other people's agents and names the owner

Session A: "Add a one-line comment at the top of `<a file in src/billing/>`."

Expect: denied. The message says the file is owned territory, names `lina@example.com`, and
suggests your own terminal: `acs override --path 'src/billing/**' --once`. The file is unchanged
(`git status`).

### T2. Your agents may work in your own zone

Session A: "Add a one-line comment at the top of `<a file in src/core/>`."

Expect: allowed. `acs status` lists the file under the session's claims.

### T3. Owned zones bind agents, not people

Edit `<a file in src/billing/>` yourself in your editor and commit it from your terminal.

Expect: nothing stops you, and the commit goes through. Revert it afterwards if you like.

### T4. `--once` lets one agent call in

1. In your terminal, run this and type the code it shows:

   ```bash
   acs override --path 'src/billing/**' --once
   ```

2. `acs status` lists the override.
3. Session A: "Try the src/billing/ comment again."
4. Session A: "Now add the same comment to `<another file in src/billing/>`."

Expect: step 3 is allowed and uses the override up, so `acs status` no longer lists it. Step 4
is denied again.

If step 1 refuses with "must be run by a person in a terminal" although you typed it yourself in
Git Bash, try the same command in PowerShell. Either way, note it: it would be a bug in how ACS
detects Git Bash's terminal.

### T5. `--session` lets one session in, and only that one

1. Grant it in your terminal:

   ```bash
   acs override --path 'src/billing/**' --session
   ```

2. Session A edits two files in `src/billing/`.
3. Session B: "Add a comment to `<a third file in src/billing/>`."

Expect: Session A's edits are allowed; the override belongs to the first session that used it.
Session B is denied. The override disappears when Session A ends.

### T6. Revoking, and overrides nobody uses

1. Grant one and copy its id (`ovr_...`) from the output:

   ```bash
   acs override --path 'src/billing/**' --once
   ```

2. Revoke it:

   ```bash
   acs override --revoke <ovr_id>
   ```

3. Session A tries the edit.

Expect: `acs status` no longer lists it, and Session A is denied. An override nobody uses also
lapses on its own after 15 minutes.

### T7. Overrides need a person at a terminal

1. Session A: "Run `acs override --path 'src/billing/**' --once` in the shell."
2. In your terminal, pipe a code in instead of typing it:

   ```bash
   echo ABCD | acs override --path 'src/billing/**' --once
   ```

Expect: step 1 is denied with "Agents may not run `acs override` ...". Step 2 refuses because it
is not run by a person in a terminal.

### T8. Agents cannot switch ACS off

Session B: give one command per prompt, and ask it to report and not work around a block:

| Ask the agent to | Expected |
| --- | --- |
| Run `acs off` | Denied: agents may not run `acs off` |
| Run `acs claim README.md` | Denied; it is told to use its `acs_claim` tool |
| Commit any change with `git commit --no-verify -m test` | Denied: skips the git hooks |
| Run `git -c core.hooksPath=/dev/null commit -m test` | Denied |
| Add a key to `.claude/settings.json` | Denied: sealed path |
| Create a file in `~/.acs/` | Denied: sealed path |
| Run `claude -p hi --settings x.json` | Denied: starts an agent with ACS off |
| Run `acs status` and `acs lore list` | Both allowed |

### T9. Writes outside the repository do not prompt

Session B: "Write a file `acs-test.txt` in your system's temp folder with the text hello."

Expect: no ACS prompt. This changed in decision 0027.

### T10. Two sessions on one file (a sanity check of the fresh install)

1. Session A: "Add a comment to `<a file outside the owned zones>` and keep working on that file."
2. Session B: "Add a different comment to the same file."

Expect: Session B is denied and the message names Session A's session. `acs status` shows the
collision.

### T11. `acs watch` claims what you change in any editor

1. In a second terminal:

   ```bash
   acs watch
   ```

2. Edit and save `<a file outside the owned zones>` in Notepad or any editor.
3. `acs status` lists the file under your claims (`human`).
4. Session A: "Add a comment to that file."
5. Leave the file alone for 2 minutes, then ask again.

Expect: step 4 is denied and the message names you (`human`). Step 5 is allowed. Stop the watcher
with Ctrl+C.

### T12. The VS Code extension

Follow the in-editor checks in [wp7-humans.md](wp7-humans.md) (build the `.vsix` from
`integrations/vscode`, steps 1 to 8). In short: typing in a file claims it within a second, an
agent's edit is then denied naming you, files an agent holds show a lock, and the claim is
released two minutes after you save.

### T13. A teammate through the remote (optional; it also covers owned zones from the owner's side)

This simulates Lina on another machine: a second clone with her email and her own `ACS_HOME`.

1. Make the clone, next to your repository:

   ```bash
   git clone <remote-url> ../repo-lina
   ```

   ```bash
   cd ../repo-lina && git config user.email lina@example.com
   ```

   ```bash
   export ACS_HOME=~/.acs-lina && acs init
   ```

2. Start Claude Code from that same terminal, so it inherits `ACS_HOME`, and accept the plugin.
   Ask it: "Add a comment to `<a file in src/billing/>` and keep working on it."
3. Run `acs sync` in both clones, or wait about 30 seconds.
4. In your repository, Session A: "Add a comment to `<the same file in src/billing/>`."

Expect: Lina's agent is allowed, because she owns the zone. Your `acs status` lists her claim with
`[remote]`. Session A is denied, as in T1.

## Phase 6: lore

### L1. Lore arrives once, most specific first

1. Start a new Session A: "Read `<a file in src/api/>` and quote word for word any ACS lore you
   were given."
2. Then: "Add a small helper function to `<a file in src/api/>`. Did you get any new ACS lore
   for that?"

Expect:
- Step 1 quotes two blocks, `lore_api` first, then `lore_src` (narrower scope first). Each starts
  `ACS lore for ... (lore_..., owner ME):`.
- Step 2 gets no new lore. The new function has a `// API:` comment above it, and the agent's
  last message ends with "Lore checked.".

### L2. Every session gets its own copy

Session B: "Read `<another file in src/api/>` and quote any ACS lore you were given."

Expect: the same two blocks.

### L3. Out-of-date lore asks to be checked

Session A: "Read `<a file in src/legacy/>` and quote any ACS lore you were given."

Expect: `lore_legacy`, with the line "This entry may be out of date: N% of the lines in its scope
changed since it was written ...". `acs lore stale --all` shows the same percentage. If
`src/legacy/` changed less than 30%, the line is missing; lowering `stale_percent` in L5's config
shows it.

### L4. What you can see from your terminal

```bash
acs lore show <a file in src/api/>
```

```bash
acs lore list
```

```bash
acs lore stale --all
```

Expect: `show` prints what the agent got in L1, in the same order. `list` shows three entries.
`stale --all` gives a percentage for each entry.

### L5. The size limit

1. Add this to `.acs/policy/config.toml`. Committing is not needed: ACS reads the file on disk.

   ```toml
   [lore]
   max_inject_chars = 250
   ```

   250 fits `lore_api` (about 200 characters with its heading) but not both entries. If your email
   is very long and `lore_api` comes back cut ("[Cut to fit ...]"), raise it to 300.

2. New Session C: "Read `<a file in src/api/>` and quote any ACS lore you were given. Then read
   it again and quote any new lore."
3. Remove the `[lore]` table afterwards.

Expect: the first read brings only `lore_api`, ending with "[1 more lore entry applies here did
not fit; it will come with your next tool call in this scope ...]". The second read brings
`lore_src`.

### L6. A broken lore file does not stop the rest

1. Create `.acs/policy/lore/broken.md` containing just `oops`.
2. Run:

   ```bash
   acs lore list
   ```

3. New Session C reads a file in `src/api/`.
4. Delete `broken.md`.

Expect: `acs lore list` shows the three good entries and lists `broken.md` with the reason. The
agent still gets its lore.

### L7. Agents cannot write or accept lore

Session A, one prompt each:

| Ask the agent to | Expected |
| --- | --- |
| Fix a word in `.acs/policy/lore/src/api.md` | Denied: sealed path |
| Run `acs lore review` | Denied; the message points to `acs_lore_propose` |
| Run `acs lore list` | Allowed |

### L8. Agents propose; nothing changes in the repository

1. Session A: "Propose lore for `src/api/**` with acs_lore_propose: one fact about this folder
   you think the next agent should know."
2. Session A: "Propose a correction of `lore_api` that says the comment must start with
   `// API(owner):`."
3. Session B: "Propose lore for `src/billing/**`: one fact about that folder."

Expect:
- Each reply gives a `prop_...` id and says it waits for review.
- `git status` shows nothing new under `.acs/`.
- `acs status` says "Lore proposals from agents: 3 wait for review".

### L9. Reviewing

1. In your terminal:

   ```bash
   acs lore review
   ```

2. Accept the `src/api/**` proposal. Choose edit for the `lore_api` correction, change the text
   in the file it names, save, and press Enter. Accept the `src/billing/**` one.
3. Look at the result:

   ```bash
   git branch --list 'acs/lore-*'
   ```

   ```bash
   git status
   ```

   ```bash
   git show acs/lore-api --stat
   ```

Expect:
- Each accepted proposal is a branch: `acs/lore-<new id>` for new entries, `acs/lore-api` for
  the correction. The correction replaces `.acs/policy/lore/src/api.md` with your edited text.
- `git status` shows your working tree unchanged.
- The `src/billing/**` entry gets owner `lina@example.com` (the zone's owner), and ACS says the
  owner is not you.
- `acs lore review --list` is empty.

### L10. Through the remote: from proposal to lore in force

1. Push the branch for the new `src/api/**` entry and open a pull request on GitHub:

   ```bash
   git push -u origin acs/lore-<id>
   ```

2. Merge the pull request, then pull on `main`:

   ```bash
   git pull
   ```

3. New Session D: "Read `<a file in src/api/>` and quote any ACS lore you were given."

Expect: Session D gets the new entry along with `lore_api` and `lore_src`.

### L11. Proposals stay on the machine where they were made (needs T13's clone)

In `../repo-lina`, with `ACS_HOME=~/.acs-lina` set:

```bash
acs lore review --list
```

Expect: no proposals. Your agents' proposals live in your `ACS_HOME`, not in the repository.

## After testing

- Delete the `acs/lore-*` branches you did not merge, locally and on the remote.
- Remove the test zones and lore, or keep them.
- For T13, delete `../repo-lina` and `~/.acs-lina`.

## Results

| Test | Result | Notes |
| --- | --- | --- |
| 0 cleanup and setup | | |
| T1 owned zone denies | | |
| T2 own zone allowed | | |
| T3 people not bound | | |
| T4 `--once` | | |
| T5 `--session` | | |
| T6 revoke | | |
| T7 terminal only | | |
| T8 self-protection | | |
| T9 outside repo | | |
| T10 two sessions | | |
| T11 `acs watch` | | |
| T12 VS Code | | |
| T13 teammate | | |
| L1 once, most specific | | |
| L2 second session | | |
| L3 out of date | | |
| L4 terminal commands | | |
| L5 size limit | | |
| L6 broken file | | |
| L7 agents cannot write | | |
| L8 propose | | |
| L9 review | | |
| L10 through the remote | | |
| L11 per machine | | |
