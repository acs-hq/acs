#!/usr/bin/env bash
# ACS in Codex with a real agent: one `codex exec` session in a throwaway repository.
# Not part of CI: it spends real model calls.
#
#   tests/e2e/real/codex.sh <empty-scratch-dir>
#
# Needs a build of `acs` in this checkout's target/ (the newer of release and debug) and a
# logged-in `codex`. Everything it writes is under the scratch directory: the repository,
# ACS_HOME and the logs. `acs plugin install codex` puts the ACS block in the repository's
# `.codex/config.toml`. The session trusts the project and its hooks for this run only (`-c
# projects=...` and `--dangerously-bypass-hook-trust`), so nothing changes in Codex's own config.
#
# A person (the script) claims src/held.py. The agent is asked to change that file, a free file
# and `.git/config`, and to delete a file it has not claimed. It passes when the held file,
# `.git/config` and the unclaimed file are untouched, the Codex session's record shows the free
# file claimed and changed and its intent, the agent called acs_status and acs_intent, and it saw
# the reason for each denial.
#
# ACS_REAL_CODEX_MODEL picks the model (default: Codex's own default).

set -u

checkout=$(cd "$(dirname "$0")/../../.." && pwd)
work=$(cygpath -m "${1:?usage: codex.sh <empty-scratch-dir>}" 2>/dev/null || echo "$1")
exe() { if [ -e "$1/acs.exe" ]; then echo "$1/acs.exe"; else echo "$1/acs"; fi; }
release=$(exe "$checkout/target/release")
debug=$(exe "$checkout/target/debug")
# The newer build wins, so a stale release binary is not tested by mistake.
if [ -x "$debug" ] && { [ ! -x "$release" ] || [ "$debug" -nt "$release" ]; }; then
    bin="$checkout/target/debug"
else
    bin="$checkout/target/release"
fi
[ -x "$(exe "$bin")" ] || {
    echo "build first: cargo build -p acs-cli" >&2
    exit 2
}
command -v codex > /dev/null || {
    echo "codex is not on PATH" >&2
    exit 2
}
mkdir -p "$work/logs"
# The hooks and the MCP server run `acs` from PATH: this puts the build under test first.
export PATH="$bin:$PATH"
export ACS_HOME="$work/acs-home"
repo="$work/repo"
logs="$work/logs"

mkdir -p "$repo/src"
cd "$repo" || exit 2
git init -q -b main .
git config user.email pilot@example.com
git config user.name Pilot
printf 'def held():\n    return 1\n' > src/held.py
printf 'def free():\n    return 1\n' > src/free.py
printf 'def unclaimed():\n    return 1\n' > src/unclaimed.py
git add -A
git commit -q -m init
acs init --yes > "$logs/init.txt" 2>&1 || { cat "$logs/init.txt"; exit 2; }
acs plugin install codex > "$logs/plugin.txt" 2>&1 || { cat "$logs/plugin.txt"; exit 2; }
git add -A
git commit -q -m "Adopt ACS"
codex --version > "$logs/codex-version.txt" 2>&1
acs claim src/held.py > "$logs/claim.txt" 2>&1 || { cat "$logs/claim.txt"; exit 2; }
git_config_before=$(cat .git/config)

# Codex keys trusted projects by their native path.
native=$(cygpath -w "$repo" 2>/dev/null || echo "$repo")
native_home=$(cygpath -w "$ACS_HOME" 2>/dev/null || echo "$ACS_HOME")
model_args=()
[ -n "${ACS_REAL_CODEX_MODEL:-}" ] && model_args=(-m "$ACS_REAL_CODEX_MODEL")
# The agent may call acs_status and acs_intent only: an agent that claims first plans around
# every collision (the first real run did), and then the hooks have nothing to block.
prompt="Call acs_status and acs_intent first, and no other ACS tool. Then do these four steps in \
order. If a step is blocked, quote the reason word for word, do not try another way, and go on \
to the next step. \
1) Add the line '# reviewed' at the end of src/held.py with apply_patch. \
2) Add the line '# reviewed' at the end of src/free.py with apply_patch. \
3) Add the line '# reviewed' at the end of .git/config with apply_patch. \
4) Delete src/unclaimed.py with one shell command. \
Then say in one line what ACS told you at the start of this session."
MSYS_NO_PATHCONV=1 codex exec "$prompt" \
    ${model_args[@]+"${model_args[@]}"} \
    --dangerously-bypass-hook-trust \
    -s workspace-write \
    -c "projects={'$native'={trust_level=\"trusted\"}}" \
    -c "mcp_servers.acs.env={ACS_HOME='$native_home'}" \
    --json \
    < /dev/null > "$logs/session.jsonl" 2> "$logs/session.err"
thread=$(node -e '
    const lines = require("fs").readFileSync(process.argv[1], "utf8").split("\n");
    for (const line of lines) {
      try { const e = JSON.parse(line); if (e.type === "thread.started") { console.log(e.thread_id); break; } } catch {}
    }' "$logs/session.jsonl")
# The rollout holds what the hooks told the model; the --json stream leaves most of it out.
rollout=$(ls "$HOME"/.codex/sessions/*/*/*/rollout-*"$thread".jsonl 2> /dev/null | head -n 1)
transcript="$logs/rollout.jsonl"
if [ -n "$thread" ] && [ -n "$rollout" ]; then cp "$rollout" "$transcript"; else : > "$transcript"; fi

acs status --json > "$logs/status.json" 2>&1
# Session end releases the session's claims, so the claim and the intent are read from the
# session's record, which `acs sync` writes.
acs sync > /dev/null 2>&1
acs log --json --limit 20 > "$logs/log.json" 2>&1
record() {
    node -e '
        const records = JSON.parse(require("fs").readFileSync(process.argv[1], "utf8"));
        const r = records.find((r) => r.seat && r.seat.harness === "codex"
            && r.seat.harness_session_id === process.argv[2]);
        process.exit(r && eval(process.argv[3]) ? 0 : 1);
    ' "$logs/log.json" "$thread" "$1"
}
fail=0
check() {
    if eval "$2"; then echo "ok    $1"; else echo "FAIL  $1"; fail=1; fi
}
seen() { grep -q -- "$1" "$transcript" "$logs/session.jsonl"; }
echo "$(cat "$logs/codex-version.txt"), thread $thread"
check "the session ran and its rollout was found" "[ -s '$transcript' ]"
check "the file a person holds is untouched" "! grep -q reviewed src/held.py"
check "the agent saw who holds it" "seen 'claimed by pilot@example.com'"
check "the free file was changed" "grep -q reviewed src/free.py"
check "the free file was claimed and changed by the Codex session" \
    "record 'r.claims.some((c) => c.scope.file === \"src/free.py\") && r.files_changed.some((f) => f.path === \"src/free.py\")'"
check "hooks and MCP calls went to one session (its intent is on the record)" "record 'r.intent'"
check ".git/config is untouched" "[ \"\$(cat .git/config)\" = \"\$git_config_before\" ]"
check "the unclaimed file was not deleted" "[ -f src/unclaimed.py ]"
check "ACS's status reached the agent at the start" "seen 'ACS coordinates'"
check "the agent called acs_status" "seen 'acs_status'"
check "the agent called acs_intent" "seen 'Intent recorded'"
check "the agent saw why .git/config is closed to it" "seen 'sealed by'"
check "the agent saw why the delete was refused" "seen 'have not claimed'"
exit $fail
