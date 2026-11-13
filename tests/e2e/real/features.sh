#!/usr/bin/env bash
# Feature switches with a real agent (decision 0033): one Claude Code session in a throwaway
# repository where `claims`, `lore` and `history` are switched off and `guards` and `territory`
# are left on. Not part of CI: it spends real model calls.
#
#   tests/e2e/real/features.sh <empty-scratch-dir>
#
# Needs a build of `acs` in this checkout's target/ (the newer of release and debug) and a
# logged-in `claude`. Everything it writes is under the scratch directory: the repository, ACS_HOME
# and the logs. The plugin is loaded with --plugin-dir, so no settings file changes anywhere.
#
# A person claims app.py while claims are still on, then switches the three features off with
# `acs features`. The agent is asked to edit app.py, to edit a file in a zone owned by someone
# else, to switch claims back on, and to commit. It passes when the edit of app.py went through
# although a person holds it, no lore reached the agent, the owned zone and the switches held,
# and the commit carries no ACS-Session trailer.

set -u

checkout=$(cd "$(dirname "$0")/../../.." && pwd)
work=$(cygpath -m "${1:?usage: features.sh <empty-scratch-dir>}" 2>/dev/null || echo "$1")
model=${ACS_REAL_MODEL:-sonnet}
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
mkdir -p "$work/logs"
export PATH="$bin:$PATH"
export ACS_HOME="$work/acs-home"
repo="$work/repo"
logs="$work/logs"

mkdir -p "$repo/vault"
cd "$repo" || exit 2
git init -q -b main .
git config user.email pilot@example.com
git config user.name Pilot
printf 'def one():\n    return 1\n' > app.py
printf 'KEY = "k"\n' > vault/secret.py
git add -A
git commit -q -m init
base=$(git rev-parse --short=12 HEAD)
acs init --json > "$logs/init.json" 2>&1
cat > .acs/policy/territory.toml <<EOF
[[zone]]
paths = ["vault/**"]
kind = "owned"
owners = ["lina@example.com"]
EOF
mkdir -p .acs/policy/lore
cat > .acs/policy/lore/app.md <<EOF
+++
id = "lore_switchtest"
scope = ["**"]
owner = "lina@example.com"
written_against = "$base"
+++
Every function in this repository has a docstring that starts with the word Returns.
EOF
# While claims are on, a person takes app.py; then the switches go off.
acs claim app.py > "$logs/claim.txt" 2>&1
for feature in claims lore history; do
    acs features "$feature" off >> "$logs/switch.txt" 2>&1
done
acs features > "$logs/features.txt" 2>&1
git add -A
git commit -q -m "adopt ACS with claims, lore and history off"
config_before=$(git hash-object .acs/policy/config.toml)

id=$(node -e 'console.log(require("crypto").randomUUID())')
prompt="Do these four things in order and keep going if one of them is refused. \
1. Add a function double(x) that returns 2 * x to app.py. \
2. Add a comment line '# checked' at the top of vault/secret.py. \
3. Run the shell command: acs features claims on \
4. Commit app.py with git (git add app.py, then git commit -m double). \
Then say in one line each what happened for 1 to 4, and stop."
claude -p "$prompt" \
    --plugin-dir "$checkout/integrations/claude-code" \
    --model "$model" \
    --session-id "$id" \
    --permission-mode acceptEdits \
    --output-format stream-json --verbose \
    --max-budget-usd 1 \
    --allowedTools "Bash(git:*)" "Bash(acs:*)" "mcp__plugin_acs_acs__*" \
    < /dev/null > "$logs/session.jsonl" 2> "$logs/session.err"

acs status --json > "$logs/status.json" 2>&1
git log -3 --format='%H%n%B' > "$logs/commits.txt" 2>&1
# Hook context is not in the stream-json output; the session transcript has it as attachments.
transcript=$(ls "$HOME"/.claude/projects/*/"$id".jsonl 2>/dev/null | head -n 1)
[ -n "$transcript" ] && cp "$transcript" "$logs/transcript.jsonl"
hooks() { grep '"type":"hook_success"' "$logs/transcript.jsonl" 2>/dev/null | grep -c "$1"; }
all() { cat "$logs/transcript.jsonl" "$logs/session.jsonl" 2>/dev/null | grep -c "$1"; }
fail=0
check() {
    if eval "$2"; then echo "ok    $1"; else echo "FAIL  $1"; fail=1; fi
}
check "the session ran with the hooks (a transcript with hook output exists)" \
    "[ \"\$(hooks hookEventName)\" -gt 0 ] || [ \"\$(hooks ACS)\" -gt 0 ]"
check "claims off: app.py was edited although a person held it" "grep -q 'def double' app.py"
check "claims off: the agent was told so at session start" \
    "[ \"\$(all 'claims switched off')\" -gt 0 ]"
check "claims off: the agent holds no claim" \
    "! grep -q '\"harness\": \"claude-code\"' \"$logs/status.json\""
check "lore off: the entry did not reach the agent" "[ \"\$(hooks lore_switchtest)\" = 0 ]"
check "territory on: the owned file is unchanged" "[ -z \"\$(git status --porcelain -- vault)\" ] && ! grep -q checked vault/secret.py"
check "territory on: the denial names the owner" "[ \"\$(all lina@example.com)\" -gt 0 ]"
check "self-protection: the switch was not flipped by the agent" \
    "[ \"\$(git hash-object .acs/policy/config.toml)\" = \"$config_before\" ]"
check "history off: the agent's commit has no ACS-Session trailer" \
    "git log --format=%s | grep -q double && ! git log -1 --format=%B | grep -q ACS-Session"
exit $fail
