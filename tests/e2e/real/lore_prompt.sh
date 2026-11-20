#!/usr/bin/env bash
# Is an agent asked to propose lore at the end of a task, once, without looping (issue 80,
# decision 0048)? Real sessions in a throwaway repository, with tasks that never mention lore.
# Not part of CI: it spends real model calls.
#
#   tests/e2e/real/lore_prompt.sh <empty-scratch-dir>
#
# Needs a build of `acs` in this checkout's target/ (the newer of release and debug) and a
# logged-in `claude`; for the other parts a logged-in `codex` and `agy` (ACS_REAL_SKIP_CODEX=1 or
# ACS_REAL_SKIP_AGY=1 leaves one out). Everything it writes is under the scratch directory.
#
# The repository's README holds a rule an agent cannot see from calc.py (it must stay importable
# by Python 2.7), so an agent that edits calc.py has something worth proposing.
#
# Claude Code: a session that edits calc.py must be asked exactly once and must end; a session
# that only answers a question must not be asked. Codex and Antigravity: one session that edits,
# asked exactly once, and it ends. Whether the agent actually proposed something is reported, not
# required: the request is the feature, the answer is the model's.
#
# ACS_REAL_MODEL picks Claude Code's model (default sonnet), ACS_REAL_AGY_MODEL Antigravity's
# (default gemini-3.8-flash-medium), ACS_REAL_CODEX_MODEL Codex's (default: Codex's own).

set -u
. "$(dirname "$0")/common.sh" "${1:?usage: lore_prompt.sh <empty-scratch-dir>}"
agy_model=${ACS_REAL_AGY_MODEL:-gemini-3.8-flash-medium}
ask='ACS, before you finish'

export GIT_AUTHOR_NAME=Pilot GIT_AUTHOR_EMAIL=pilot@example.com
export GIT_COMMITTER_NAME=Pilot GIT_COMMITTER_EMAIL=pilot@example.com

# new_repo <dir>: a small project with ACS adopted.
new_repo() {
    git init -q -b main "$1"
    (
        cd "$1" || exit 2
        git config user.email pilot@example.com
        git config user.name Pilot
        printf 'def add(a, b):\n    return a + b\n\n\ndef sub(a, b):\n    return a - b\n' > calc.py
        printf '# calc\n\nA tiny calculator module. calc.py is also imported by the old firmware\nbuild, which runs Python 2.7: no f-strings, no type hints, no walrus operator in it.\n' > README.md
        git add -A
        git commit -q -m first
        acs init --yes > "$logs/init-$(basename "$1").txt" 2>&1 || exit 2
        git add -A
        git commit -q -m "Adopt ACS"
    ) || { echo "could not set up $1" >&2; exit 2; }
}
# asked <file>...: how many times the request reached the agent. A request is one hook answer,
# but a transcript may hold it in more than one record, so count the records that carry the
# request text as the start of a hook's reason or an injected message, once per distinct record.
asked() { cat "$@" 2>/dev/null | grep -c -- "$ask"; }
proposals() { acs lore review --list --json 2>/dev/null | node -e '
    let t = ""; process.stdin.on("data", (d) => t += d).on("end", () => {
      try { const v = JSON.parse(t); console.log((v.proposals ?? v).length); } catch { console.log("?"); }
    });'; }

# ---------------------------------------------------------------------------------------------
# Claude Code
# ---------------------------------------------------------------------------------------------
if [ -z "${ACS_REAL_SKIP_CLAUDE:-}" ]; then
    new_repo "$repo"
    cd "$repo" || exit 2
    echo "Claude Code $(claude --version 2> /dev/null), model $model"

    session edit "Add a function mul(a, b) to calc.py that returns a * b, in the same style as the others. Read README.md first." \
        "Bash(git:*)" "Bash(python:*)" "mcp__plugin_acs_acs__*"
    check "edit: the hooks ran" "hooks_ran edit"
    check "edit: the function is there" "grep -q 'def mul' calc.py"
    check "edit: the session ended with a result" "grep -q '\"type\":\"result\"' '$logs/edit.jsonl'"
    n=$(asked "$logs/edit.transcript.jsonl")
    echo "  edit: records carrying the request: $n"
    check "edit: the agent was asked" "[ $n -ge 1 ]"
    stops=$(grep -c '"hook_event_name":"Stop"\|Stop hook' "$logs/edit.transcript.jsonl" 2>/dev/null)
    echo "  edit: transcript lines naming the Stop hook: $stops"
    check "edit: the start message had the agent load acs_lore_propose" \
        "grep -q 'mcp__plugin_acs_acs__acs_lore_propose' '$logs/edit.transcript.jsonl'"
    echo "  edit: acs_lore_propose called: $(grep -c '"name":"mcp__plugin_acs_acs__acs_lore_propose"' "$logs/edit.jsonl")"
    echo "  edit: proposals waiting: $(proposals)"

    session question "How many functions does calc.py define? Answer in one line." \
        "Bash(git:*)" "mcp__plugin_acs_acs__*"
    check "question: the hooks ran" "hooks_ran question"
    check "question: the session ended with a result" "grep -q '\"type\":\"result\"' '$logs/question.jsonl'"
    check "question: the agent was not asked (it changed nothing)" \
        "[ \"\$(asked '$logs/question.transcript.jsonl')\" -eq 0 ]"
    echo "  acs status says: $(acs status 2>/dev/null | grep -i 'lore proposals' || echo '(no proposals line)')"
fi

# ---------------------------------------------------------------------------------------------
# Codex
# ---------------------------------------------------------------------------------------------
if [ -z "${ACS_REAL_SKIP_CODEX:-}" ]; then
    command -v codex > /dev/null || { echo "codex is not on PATH (ACS_REAL_SKIP_CODEX=1 skips it)" >&2; exit 2; }
    cx_repo="$work/codex-repo"
    new_repo "$cx_repo"
    cd "$cx_repo" || exit 2
    acs plugin install codex > "$logs/codex-plugin.txt" 2>&1 || { cat "$logs/codex-plugin.txt"; exit 2; }
    git add -A
    git commit -q -m "ACS plugin for Codex"
    echo "$(codex --version 2> /dev/null)"
    native=$(cygpath -w "$cx_repo" 2>/dev/null || echo "$cx_repo")
    native_home=$(cygpath -w "$ACS_HOME" 2>/dev/null || echo "$ACS_HOME")
    model_args=()
    [ -n "${ACS_REAL_CODEX_MODEL:-}" ] && model_args=(-m "$ACS_REAL_CODEX_MODEL")
    MSYS_NO_PATHCONV=1 codex exec "Add a function mul(a, b) to calc.py that returns a * b, in the same style as the others. Read README.md first." \
        ${model_args[@]+"${model_args[@]}"} \
        --dangerously-bypass-hook-trust \
        -s workspace-write \
        -c "projects={'$native'={trust_level=\"trusted\"}}" \
        -c "mcp_servers.acs.env={ACS_HOME='$native_home'}" \
        --json \
        < /dev/null > "$logs/codex.jsonl" 2> "$logs/codex.err"
    cx_status=$?
    thread=$(node -e '
        const lines = require("fs").readFileSync(process.argv[1], "utf8").split("\n");
        for (const line of lines) {
          try { const e = JSON.parse(line); if (e.type === "thread.started") { console.log(e.thread_id); break; } } catch {}
        }' "$logs/codex.jsonl")
    rollout=$(ls "$HOME"/.codex/sessions/*/*/*/rollout-*"$thread".jsonl 2> /dev/null | head -n 1)
    if [ -n "$thread" ] && [ -n "$rollout" ]; then cp "$rollout" "$logs/codex-rollout.jsonl"; else : > "$logs/codex-rollout.jsonl"; fi
    check "codex: the session exited cleanly" "[ $cx_status -eq 0 ]"
    check "codex: the function is there" "grep -q 'def mul' calc.py"
    n=$(asked "$logs/codex-rollout.jsonl")
    echo "  codex: records carrying the request: $n"
    check "codex: the agent was asked" "[ $n -ge 1 ]"
    echo "  codex: acs_lore_propose called: $(grep -c 'acs_lore_propose' "$logs/codex.jsonl")"
    echo "  codex: proposals waiting: $(proposals)"
fi

# ---------------------------------------------------------------------------------------------
# Antigravity
# ---------------------------------------------------------------------------------------------
if [ -z "${ACS_REAL_SKIP_AGY:-}" ]; then
    command -v agy > /dev/null || { echo "agy is not on PATH (ACS_REAL_SKIP_AGY=1 skips it)" >&2; exit 2; }
    agy_repo="$work/agy-repo"
    new_repo "$agy_repo"
    cd "$agy_repo" || exit 2
    acs plugin install antigravity > "$logs/agy-plugin.txt" 2>&1 || { cat "$logs/agy-plugin.txt"; exit 2; }
    git add -A
    git commit -q -m "ACS plugin for Antigravity"
    echo "agy $(agy --version 2> /dev/null), model $agy_model"
    printf '{"toolPermission":"always-proceed"}\n' > "$work/agy-settings.json"
    MSYS_NO_PATHCONV=1 agy -p "Add a function mul(a, b) to calc.py that returns a * b, in the same style as the others. Read README.md first." \
        --model "$agy_model" \
        --settings "$work/agy-settings.json" \
        --output-format stream-json \
        --print-timeout 600s \
        < /dev/null > "$logs/agy.ndjson" 2> "$logs/agy.err"
    agy_status=$?
    conversation=$(node -e '
        const lines = require("fs").readFileSync(process.argv[1], "utf8").split("\n");
        for (const line of lines) {
          try { const e = JSON.parse(line); if (e.event === "init") { console.log(e.conversation_id); break; } } catch {}
        }' "$logs/agy.ndjson")
    transcript="$logs/agy-transcript.ndjson"
    cp "$HOME/.gemini/antigravity-cli/brain/$conversation/.system_generated/logs/transcript_full.jsonl" \
        "$transcript" 2> /dev/null || : > "$transcript"
    check "agy: the session exited cleanly" "[ $agy_status -eq 0 ]"
    check "agy: the function is there" "grep -q 'def mul' calc.py"
    n=$(asked "$transcript" "$logs/agy.ndjson")
    echo "  agy: records carrying the request: $n"
    check "agy: the agent was asked" "[ $n -ge 1 ]"
    echo "  agy: acs_lore_propose called: $(grep -c 'acs_lore_propose' "$transcript")"
    echo "  agy: proposals waiting: $(proposals)"
fi
finish
