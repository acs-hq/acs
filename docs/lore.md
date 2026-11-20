# Lore

Lore is what you would tell a new colleague before they touch a part of your repository: "money is
integer cents here", "never call Stripe directly", "this folder is generated, edit the template".
ACS shows it to every agent the first time the agent works in that part of the repository, once
per session.

## A worked example

Lina owns `payments/`. She writes `.acs/policy/lore/payments/money.md`:

```markdown
+++
id = "lore_cents"
scope = ["payments/**"]
owner = "lina@example.com"
written_against = "3f2a9c1d7b44"
+++
Money is an int number of cents, never a float. Name money variables `*_cents`.
```

and merges it like any other change. Then:

1. Omar's agent opens `payments/refund.py` to read it. Along with the file, ACS adds this to the
   agent's context:

   ```
   ACS lore for `payments/**` (lore_cents, owner lina@example.com):
   Money is an int number of cents, never a float. Name money variables `*_cents`.
   ```

2. The agent edits three more files in `payments/`. It is not told again: once per session.
3. Weeks later most of `payments/` has been rewritten. The next agent still gets the entry, with a
   line asking it to verify the entry against the code first, and `acs lore stale` lists it for
   Lina.
4. Another agent of Omar's fixes a bug in `payments/refund.py` and learns on the way that refunds
   must be idempotent. When it finishes its turn, ACS asks it once whether it learned something
   the next agent needs. It calls `acs_lore_propose` with `scope = ["payments/**"]` and the text,
   and its closing message tells Omar that a proposal waits. Nothing changes in the repository:
   the proposal waits on Omar's machine, and `acs status` (and the VS Code status bar) shows it.
5. Omar runs `acs lore review`, reads the proposal and accepts it. ACS commits the new entry on a
   branch `acs/lore-<id>`, without touching his work tree, and he opens a pull request for Lina to
   review.

Agents can read lore but never write it: `.acs/` is sealed, and `acs lore review` is denied to
them.

## How agents are asked to propose

Where `lore` is on, ACS asks an agent once per session, at the end of the first turn in which it
changed files (Claude Code's and Codex's `Stop` hook, Antigravity's `Stop` hook): did you learn
something about this code that the next agent needs? The agent gets one more turn to answer, by
proposing or by stopping; it is never asked twice. A session that only read files, or answered a
question, is not asked. In Claude Code the session-start message also has the agent load
`acs_lore_propose` along with `acs_status` and `acs_intent`, so the tool is at hand. Where lore is
off, nothing of this happens and the tool is not listed (decision 0034).

## Lore and hand-off files

Many repositories already have a place where agents leave notes: `CLAUDE.md`, `AGENTS.md`, a
hand-off file, issues. They hold something different from lore:

| Lore | Hand-off notes, issues |
| --- | --- |
| A fact tied to code paths: "amounts in `payments/` are integer cents" | The state of a piece of work: "the migration is half done, step 3 is next" |
| True for every agent and every later session, until the code changes | True for the next session on this work, then out of date |
| Shown automatically when an agent touches the paths | Read when an agent is pointed at it |
| Reviewed by the code's owner, committed under `.acs/policy/lore/` | Written by whoever works on it |

Agents are told the same when ACS asks them to propose, and in the usage skill.

## Writing entries

- One Markdown file per entry, anywhere under `.acs/policy/lore/` (mirroring the source tree
  keeps it findable). Files named `README*` are skipped.
- The front matter needs `id` (`lore_` and lowercase letters, digits, `_` or `-`), `scope` (path
  globs relative to the repository root, as in `territory.toml`), `owner` (an email or
  `team:<name>`) and `written_against` (the commit the text describes; `git rev-parse --short=12
  HEAD`).
- Keep entries short. Several entries can apply to one file; the most specific scope comes first,
  and one tool call adds at most `[lore] max_inject_chars` characters (4,000 by default). The rest
  comes with the agent's next calls there.
- After you update an entry, update `written_against` too: an entry is stale once
  `[lore] stale_percent` (30% by default) of the lines in its scope changed since that commit.

```toml
# .acs/policy/config.toml
spec = "acs/0.1"

[lore]
max_inject_chars = 4000
stale_percent = 30
```

## Commands

| Command | What it does |
| --- | --- |
| `acs lore list` | Entries, broken lore files (with the reason), and how many proposals wait |
| `acs lore show <path>...` | The lore for those files, as an agent working on them gets it |
| `acs lore stale [--all]` | Entries whose scope changed since they were written |
| `acs lore review` | Go through proposals one by one: accept, edit, reject or skip |
| `acs lore review --list` | List proposals |
| `acs lore review --accept <id> [--body-file F] [--owner O]` | Accept one, optionally with your own text or owner |
| `acs lore review --reject <id>` | Delete one |

Every command takes `--json`. An accepted proposal is written with the replaced entry's owner,
else the owner of the zone its scope falls in, else you; if that is not you, ACS says so, and the
owner should review the branch. Protect `.acs/policy/` with CODEOWNERS so lore changes need the
owners' approval: lore is text every agent will follow.
