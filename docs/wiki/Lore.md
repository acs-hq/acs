# Lore

Lore is what you would tell a new colleague before they touch part of your repository: "money is integer cents here", "never call Stripe directly", "this folder is generated, edit the template instead". You write it once, and ACS hands it to every agent the first time the agent works in that part of the repository.

## How it works, by example

Lina looks after `payments/`. She adds the file `.acs/policy/lore/payments/money.md`:

```markdown
+++
id = "lore_cents"
scope = ["payments/**"]
owner = "lina@example.com"
written_against = "3f2a9c1d7b44"
+++
Money is an int number of cents, never a float. Name money variables `*_cents`.
```

She commits it and merges it like any other change. From then on:

1. **An agent reads `payments/refund.py`.** Along with the file, ACS gives the agent this:

   ```
   ACS lore for `payments/**` (lore_cents, owner lina@example.com):
   Money is an int number of cents, never a float. Name money variables `*_cents`.
   ```

2. **The agent edits three more files in `payments/`.** It is not told again. Each entry comes once per session.
3. **Another session starts.** It gets the entry the first time it works in `payments/`.
4. **Months later, most of `payments/` has been rewritten.** Agents still get the entry, with a line asking them to check it against the code before relying on it. `acs lore stale` lists it, so Lina knows to update it.
5. **An agent learns something worth keeping.** At the end of its task it suggests new lore: refunds must be idempotent. Nothing changes in the repository. The suggestion waits on the machine where the agent ran.
6. **Its pilot reviews it.** They run `acs lore review` and accept it. ACS commits the new entry on a branch of its own, and they open a pull request for Lina to approve.

Agents can read lore but never write it. `.acs/` is sealed, and agents are not allowed to run `acs lore review`.

## When agents get lore

- **The first time a session touches a file in the entry's scope.** Reading, editing, claiming and shell commands that name the file all count. Lore arrives with the first read, before the agent plans its edit.
- **Once per session.** A new session gets it again.
- **Most specific first.** If `payments/**` and `payments/api/**` both have lore, an agent in `payments/api/` gets the `payments/api/**` entry first, then the other.
- **Within a size limit.** One tool call adds at most 4,000 characters of lore. What does not fit comes with the agent's next call in that scope, and the agent is told more is coming.
- **Not when the call is blocked.** A blocked agent gets the lore with its next call that goes through.

## Writing lore

Lore files go anywhere under `.acs/policy/lore/`. Mirroring your source folders keeps them easy to find: lore for `payments/` in `.acs/policy/lore/payments/`. Files named `README` are skipped, so you can explain your conventions there.

Each file starts with these settings between two `+++` lines, then the text:

| Setting | What to put |
| --- | --- |
| `id` | A name that never changes: `lore_` followed by lowercase letters, digits, `_` or `-`. Each entry needs its own. |
| `scope` | The paths it applies to, written as in [territory.toml](Configuring-Policy#how-to-write-paths), such as `["payments/**"]` |
| `owner` | Who keeps it up to date: an email, or `team:<name>` from `territory.toml` |
| `written_against` | The commit the text describes. Get it with `git rev-parse --short=12 HEAD`. |

Tips:

- **Keep entries short.** Write what an agent cannot easily learn from the code: conventions, traps and the reasons behind odd-looking code. Every agent working there reads every word.
- **One subject per entry.** Two short entries are easier to keep current than one long one.
- **Update `written_against` when you update the text.** It is how ACS tells whether an entry is out of date.
- **Check your work** with `acs lore list`. A broken file is listed with the reason and left out, and the other entries still apply.

Like the rest of the policy, commit lore from your own terminal. Agents cannot change it.

## When lore is out of date

ACS compares the entry's `written_against` commit with your latest commit, in the files its `scope` covers. Once 30% of their lines have changed, the entry counts as out of date. Only committed changes count.

An out-of-date entry is still given to agents, with a line asking them to check it against the code first and to suggest a correction if it is wrong. An entry written against a commit your clone does not have also counts as out of date.

To see which entries need attention:

```bash
acs lore stale
```

```bash
acs lore stale --all
```

The first lists only the out-of-date entries. With `--all` it lists every entry with how much of its scope has changed. To fix an entry, update the text and its `written_against`, and commit.

## Reviewing what agents suggest

Agents suggest lore with their ACS tools, either a new entry or a correction of one they were shown. Suggestions stay on the machine where the agent ran, outside the repository, until a person looks at them. `acs status` shows how many are waiting.

To go through them one by one:

```bash
acs lore review
```

For each suggestion ACS shows who sent it, where it applies and the text, then asks: accept, edit, reject, skip or quit. Edit writes the text to a file for you to change in your own editor, then accepts your version.

When you accept a suggestion:

- **ACS commits it on a new branch, `acs/lore-<id>`,** on top of your current commit. Your files, your staged changes and your current branch are not touched.
- **To put it in force,** merge the branch, or push it and open a pull request:

  ```bash
  git push -u origin acs/lore-<id>
  ```

- **The owner is filled in for you.** A correction keeps the entry's owner. A new entry gets the owner of the zone its scope falls in, or you if there is none. If the owner is not you, ACS says so: ask them to review the pull request.
- **A correction replaces the entry in place,** keeping its id and file.

Rejecting a suggestion deletes it.

To work without prompts, for example in a script:

```bash
acs lore review --list
```

```bash
acs lore review --accept prop_k2m4x7
```

```bash
acs lore review --accept prop_k2m4x7 --body-file edited.md
```

```bash
acs lore review --reject prop_k2m4x7
```

`--body-file` uses your own text instead of the suggested text. `--owner` sets a different owner.

## Commands

| Command | What it does |
| --- | --- |
| `acs lore list` | Lists the entries, broken lore files with the reason, and how many suggestions wait |
| `acs lore show <path>` | Shows the lore for a file or folder, as an agent working there gets it |
| `acs lore stale` | Lists entries whose code changed a lot since they were written |
| `acs lore review` | Goes through agents' suggestions: accept, edit, reject or skip |

Every command takes `--json`. Agents may run `list`, `show` and `stale`, but not `review`.

## Settings

Two settings in `config.toml` control lore. See [config.toml](Configuring-Policy#configtoml).

```toml
spec = "acs/0.1"

[lore]
max_inject_chars = 4000
stale_percent = 30
```

| Key | Default | Meaning |
| --- | --- | --- |
| `[lore] max_inject_chars` | `4000` | The most characters of lore one tool call adds to an agent's context |
| `[lore] stale_percent` | `30` | The share of changed lines, from 1 to 100, at which an entry counts as out of date |

## Things to know

- **Agents follow lore, so review it like code.** An entry that says something wrong or harmful is followed by every agent working there. Require the owners' approval for changes to `.acs/policy/` with a `CODEOWNERS` file.
- **`/clear` and compaction drop lore.** After you clear a session or Claude Code compacts it, the agent no longer has the lore it was shown, and ACS does not give it again in that session. Start a new session to get it back, or ask the agent to run `acs lore show` on the files it is working on.
- **Suggestions are per machine.** An agent's suggestion waits on the machine where it ran. Review it there.
