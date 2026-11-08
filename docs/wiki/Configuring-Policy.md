# Configuring policy

Policy is the set of rules for a repository. It lives in three files in `.acs/policy/`, which `acs init` creates with every setting commented out:

| File | What it controls |
| --- | --- |
| `config.toml` | General settings, such as how long a claim lasts |
| `guards.toml` | Sealed paths, protected branches and dangerous commands |
| `territory.toml` | Zones: different rules for different folders |

The policy is committed, so it is the same for every clone. Agents cannot change it: `.acs/**` is sealed, which means no agent may change it. Edit these files yourself and commit them from your own terminal. After each edit, check that the file loads:

```bash
acs doctor
```

Look for `loads without warnings` on the `Policy` line. If a file is invalid, ACS warns you and keeps using the last valid policy, or the built-in defaults if there is none.

## config.toml

| Key | Default | Meaning |
| --- | --- | --- |
| `spec` | required | Leave it as `"acs/0.1"` |
| `default_target_branch` | `"main"` | The branch your work ends up in |
| `claim_ttl_secs` | `900` | Seconds of silence before a session loses its claims. See [Claims](Working-with-ACS#claims). |
| `sync_interval_secs` | `30` | Seconds between syncs of claims with the git remote |

For example, for a team that integrates into `develop` and wants claims to last 30 minutes:

```toml
spec = "acs/0.1"
default_target_branch = "develop"
claim_ttl_secs = 1800
sync_interval_secs = 60
```

The file also has a commented `[ledger]` section. Leave it as it is.

## guards.toml

| Key | Default | Meaning |
| --- | --- | --- |
| `sealed` | The built-in list, shown in the comments of the file | Paths no agent may write, on top of the built-in list |
| `protected_branches` | `["main", "release/*"]` | Branches where agents cannot rewrite history or force push |
| `protect_outside_repo` | `false` | Ask you before an agent writes or deletes anything outside the repository |
| `[destructive] max_files_deleted_outside_claims` | `0` | How many files outside its claims a command may delete |
| `[destructive] max_files_deleted_per_command` | `50` | Above this, one command needs your approval |
| `[destructive] ban` | `["git push --force*", "git reset --hard*", "git clean -f*"]` | Commands that are always denied |

**`sealed` adds to the built-in list.** List only the paths you want to add. The built-in paths cannot be removed, even if you leave them out, so `.acs/**` always stays sealed. The comment at the top of `guards.toml` says setting a key replaces its default, but that is not true for `sealed`. `acs status --sealed` shows the full list.

**ACS guards the repository, not the rest of your machine.** By default an agent writing outside the repository (a temp folder, another project) is not ACS's business, and your harness's own permission prompts still apply. ACS still asks when it cannot tell where a path leads, or when a path outside leads back into the repository through a link, and it always refuses writes to its own state and to your harness's settings. Set `protect_outside_repo = true` to be asked about every write outside the repository.

**Copy the other defaults you want to keep.** Setting `protected_branches` or `ban` replaces its default, so anything you leave out is gone. If your `ban` list does not include `git push --force*`, that command is no longer banned.

This example seals a Terraform folder, protects `develop` as well, lowers the delete limit and bans `terraform apply`. `sealed` lists only the new path, and the other defaults are copied in:

```toml
sealed = ["infra/terraform/**"]
protected_branches = ["main", "develop", "release/*"]

[destructive]
max_files_deleted_per_command = 20
ban = [
  "git push --force*",
  "git reset --hard*",
  "git clean -f*",
  "terraform apply*",
]
```

If your hooks live in a folder such as `.githooks/`, add `".githooks/**"` to `sealed`. Only `.husky/**` is in the built-in list.

## territory.toml

Without zones, the whole repository is `shared`: an agent may edit a file once it holds a claim, and editing claims it automatically. Most repositories can leave this file alone.

A zone gives some paths a different rule:

| Kind | Effect on agents |
| --- | --- |
| `shared` | A claim is needed. This is the default. |
| `open` | No claim is needed. Agents can edit at the same time, so use it for files where that does no harm. |
| `sealed` | No agent may change these paths. |
| `owned` | Only the agents of the owners may change these paths, and they still need a claim. Any other agent is denied, and the denial names the owners. |

`owners` lists git email addresses, or `team:<name>` for a team in the `[teams]` section. An agent counts as an owner's agent when its pilot (the git `user.email` of the person running it) is one of them; case does not matter. Owned zones bind agents only: you can still edit and `acs claim` those files yourself.

To let another agent into an owned zone for a while, grant an override from your own terminal (see [Overrides](Working-with-ACS#letting-an-agent-into-owned-territory)):

```bash
acs override --path 'payments/**' --once
```

### Zones `acs init` proposes

When `territory.toml` does not exist yet, or still holds the commented default, `acs init` proposes zones. It never calls an AI model; it looks at two things:

- **Folder names.** Top-level and second-level folders named `docs`, `doc`, `documentation`, `examples`, `example`, `samples`, `sandbox`, `playground`, `scratch` or `tmp` are proposed as `open`.
- **Git history.** For each top-level folder, and each second-level folder under a top-level one with no clear owner, it counts the commits of the last 12 months that touched it, by author. A folder with at least 10 commits where one person made at least 75% of them is proposed as `owned` by that person. Owned zones are only proposed when at least two people committed in that time. Bots and `noreply@` addresses are left out.

Everything else stays `shared`. `--min-share` and `--min-commits` change the two thresholds.

`acs init` always shows the proposal, but writes it only when you say yes. You can say yes in two ways.

**In your own terminal**, `acs init` asks, but only the first time, when `territory.toml` does not exist yet:

```text
Proposed territory
    open   docs/      folder name
    owned  payments/  sara@example.com, 10 of 10 commits in the last 12 months
  Only the owners' agents may change owned zones; `acs override` lets others in.

Write these zones to .acs/policy/territory.toml? [y/N]
```

Type `y` to write the zones. Press Enter, or type anything else, to get the commented default with no zones.

**When an agent runs it, or with `--json`**, nothing can answer a question, so `acs init` does not ask. It writes the commented default and shows the proposal in its output:

```text
Territory (territory.toml)
  proposed from folder names and git history, not written:
    open   docs/      folder name
    owned  payments/  sara@example.com, 10 of 10 commits in the last 12 months
  Only the owners' agents may change owned zones; `acs override` lets others in.
  To write it, run `acs init --yes`, then edit the file as you like.
```

To accept it, run:

```bash
acs init --yes
```

During setup, the agent shows you this proposal and runs `acs init --yes` if you accept it.

Either way, `territory.toml` then holds the zones, each with a comment saying why it was proposed:

```toml
# docs/: folder name
[[zone]]
paths = ["docs/**"]
kind = "open"

# payments/: 10 of 10 commits in the last 12 months
[[zone]]
paths = ["payments/**"]
kind = "owned"
owners = ["sara@example.com"]
```

Treat this as a starting point and edit it like any other zone.

`--yes` only replaces a `territory.toml` that still holds the untouched default. Once you have edited the file, or zones have been written, `acs init --yes` leaves it alone, so running `acs init` again never overwrites your own changes.

When zones overlap, the most specific one wins. In this example everything under `docs/` is open, except `docs/legal/`, which still needs a claim:

```toml
[[zone]]
paths = ["docs/**", "scratch/**"]
kind = "open"

[[zone]]
paths = ["migrations/**"]
kind = "sealed"

[[zone]]
paths = ["docs/legal/**"]
kind = "shared"
```

Sealed zones only stop edits ACS can see. See [Limits](Troubleshooting-and-Limits#limits).

## How to write paths

Paths are relative to the root of the repository.

| Pattern | Matches |
| --- | --- |
| `docs/*` | Files directly in `docs/`. `*` never crosses a `/`. |
| `docs/**` | Everything under `docs/`, at any depth |
| `**/.env*` | Any file starting with `.env`, in any folder. A pattern only matches at any depth if it says so. |
| `{src,lib}/generated/**` | Both `src/generated/` and `lib/generated/` |

There is no way to exclude a path from a pattern. Use a more specific zone instead.

Banned commands work differently: there, `*` matches anything, including spaces and `/`.
