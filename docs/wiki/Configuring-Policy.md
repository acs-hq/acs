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
| `[destructive] max_files_deleted_outside_claims` | `0` | How many files outside its claims a command may delete |
| `[destructive] max_files_deleted_per_command` | `50` | Above this, one command needs your approval |
| `[destructive] ban` | `["git push --force*", "git reset --hard*", "git clean -f*"]` | Commands that are always denied |

**`sealed` adds to the built-in list.** List only the paths you want to add. The built-in paths cannot be removed, even if you leave them out, so `.acs/**` always stays sealed. The comment at the top of `guards.toml` says setting a key replaces its default, but that is not true for `sealed`. `acs status --sealed` shows the full list.

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
| `owned` | **Not enforced yet.** It behaves like `shared`. |

The comment in `territory.toml` says `owned` limits a path to the owners' agents. It does not do that today, so any agent can claim and edit an `owned` path. The `owners` key and the `[teams]` section have no effect yet either.

### Zones `acs init` proposes

When `territory.toml` does not exist yet, or still holds the commented default, `acs init` proposes zones. It never calls an AI model; it looks at two things:

- **Folder names.** Top-level and second-level folders named `docs`, `doc`, `documentation`, `examples`, `example`, `samples`, `sandbox`, `playground`, `scratch` or `tmp` are proposed as `open`.
- **Git history.** For each top-level folder, and each second-level folder under a top-level one with no clear owner, it counts the commits of the last 12 months that touched it, by author. A folder with at least 10 commits where one person made at least 75% of them is proposed as `owned` by that person. Owned zones are only proposed when at least two people committed in that time. Bots and `noreply@` addresses are left out.

Everything else stays `shared`. `acs init` prints the proposal but only writes it if you run `acs init --yes`, or answer yes when it asks in a terminal. `--yes` also replaces a `territory.toml` that still holds the untouched default, but never one you have edited. `--min-share` and `--min-commits` change the two thresholds. Each proposed zone has a comment saying why it was proposed. Treat the result as a starting point and edit it like any other zone.

```bash
acs init --yes
```

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
