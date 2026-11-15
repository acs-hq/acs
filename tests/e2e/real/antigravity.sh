#!/usr/bin/env bash
# The Antigravity plugin with a real agent: one `agy -p` session in a throwaway repository.
# Not part of CI: it spends real model calls.
#
#   tests/e2e/real/antigravity.sh <empty-scratch-dir>
#
# Needs a build of `acs` in this checkout's target/ (the newer of release and debug) and a
# logged-in `agy`. Everything it writes is under the scratch directory: the repository, ACS_HOME
# and the logs. `acs plugin install antigravity` puts the plugin in the repository's
# `.agents/plugins/acs/`, and the permission mode comes from a session-only `--settings` file, so
# nothing changes in agy's own settings.
#
# A person (the script) claims src/held.py. The agent is asked to change that file, a free file
# and `.git/config`, and to delete a file it has not claimed. It passes when the plugin validates,
# the held file, `.git/config` and the unclaimed file are untouched, the free file is changed and
# claimed by the agent, and the agent saw ACS's status and the reason for each denial.
#
# ACS_REAL_AGY_MODEL picks the model (default gemini-3.8-flash-medium).

set -u

checkout=$(cd "$(dirname "$0")/../../.." && pwd)
work=$(cygpath -m "${1:?usage: antigravity.sh <empty-scratch-dir>}" 2>/dev/null || echo "$1")
model=${ACS_REAL_AGY_MODEL:-gemini-3.8-flash-medium}
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
command -v agy > /dev/null || {
    echo "agy is not on PATH" >&2
    exit 2
}
mkdir -p "$work/logs"
# The plugin's hooks run `acs` from PATH: this puts the build under test first.
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
acs plugin install antigravity > "$logs/plugin.txt" 2>&1 || { cat "$logs/plugin.txt"; exit 2; }
git add -A
git commit -q -m "Adopt ACS"
agy --version > "$logs/agy-version.txt" 2>&1
agy plugin validate --strict .agents/plugins/acs > "$logs/validate.txt" 2>&1
validated=$?
acs claim src/held.py > "$logs/claim.txt" 2>&1 || { cat "$logs/claim.txt"; exit 2; }
git_config_before=$(cat .git/config)

printf '{"toolPermission":"always-proceed"}\n' > "$work/agy-settings.json"
prompt="Do these four steps in order. If a step is blocked, quote the reason word for word, do \
not try another way, and go on to the next step. \
1) Add the line '# reviewed' at the end of src/held.py with your file edit tool. \
2) Add the line '# reviewed' at the end of src/free.py with your file edit tool. \
3) Add the line '# reviewed' at the end of .git/config with your file edit tool. \
4) Delete src/unclaimed.py with one shell command. \
Then say in one line what ACS told you at the start of this conversation."
MSYS_NO_PATHCONV=1 agy -p "$prompt" \
    --model "$model" \
    --settings "$work/agy-settings.json" \
    --output-format stream-json \
    --print-timeout 600s \
    < /dev/null > "$logs/session.ndjson" 2> "$logs/session.err"
conversation=$(node -e '
    const lines = require("fs").readFileSync(process.argv[1], "utf8").split("\n");
    for (const line of lines) {
      try { const e = JSON.parse(line); if (e.event === "init") { console.log(e.conversation_id); break; } } catch {}
    }' "$logs/session.ndjson")
# The stream leaves out tool arguments and hook answers; the conversation's transcript has them.
transcript="$logs/transcript.ndjson"
cp "$HOME/.gemini/antigravity-cli/brain/$conversation/.system_generated/logs/transcript_full.jsonl" \
    "$transcript" 2> /dev/null || : > "$transcript"

acs status --json > "$logs/status.json" 2>&1
fail=0
check() {
    if eval "$2"; then echo "ok    $1"; else echo "FAIL  $1"; fail=1; fi
}
seen() { grep -q "$1" "$transcript" "$logs/session.ndjson"; }
echo "agy $(cat "$logs/agy-version.txt"), model $model"
check "agy plugin validate --strict passes" "[ $validated -eq 0 ]"
check "the file a person holds is untouched" "! grep -q reviewed src/held.py"
check "the agent saw who holds it" "seen 'pilot@example.com'"
check "the free file was changed" "grep -q reviewed src/free.py"
check "the free file is claimed by the Antigravity session" \
    "grep -q 'src/free.py' '$logs/status.json' && grep -q antigravity '$logs/status.json'"
check ".git/config is untouched" "[ \"\$(cat .git/config)\" = \"\$git_config_before\" ]"
check "the unclaimed file was not deleted" "[ -f src/unclaimed.py ]"
check "ACS's status reached the agent at the start" "seen 'ACS coordinates'"
check "the denials came from the plugin's hook" "seen 'denied by pre-tool hook'"
check "the agent saw why .git/config is closed to it" "seen 'sealed by'"
check "the agent saw why the delete was refused" "seen 'have not claimed'"
exit $fail
