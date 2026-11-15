#!/usr/bin/env bash
# Phase 4 with real agents: `acs log`, `acs blame`, `acs stats`, `acs undo` and `acs verify` after
# two Claude Code sessions changed the same files, and a person's commit against a live claim.
# Not part of CI: it spends real model calls.
#
#   tests/e2e/real/history.sh <empty-scratch-dir>
#
# Needs a build of `acs` in this checkout's target/ (the newer of release and debug) and a
# logged-in `claude`. Everything it writes is under the scratch directory: the repository,
# ACS_HOME and the logs.
#
# Session A adds a function to calc.py and a line to notes.md; session B, after it, does the same
# in the same two files. Neither commits. It passes when:
#   - both sessions are in `acs log`, and `acs blame` gives each new function to its session;
#   - `acs undo --last` names B and changes nothing; with `--apply` both files are byte-identical
#     to what A left;
#   - the person's commit of A's work carries A's trailer and `acs verify` passes;
#   - `acs undo <A> --apply` then takes A's committed work back with a new commit, rewriting
#     nothing;
#   - while session C holds shared.py, the person's commit of that file is rejected.

set -u
. "$(dirname "$0")/common.sh" "${1:?usage: history.sh <empty-scratch-dir>}"

mkdir -p "$repo"
cd "$repo" || exit 2
git init -q -b main .
git config user.email pilot@example.com
git config user.name Pilot
cat > calc.py <<'EOF'
"""Prices are integer cents."""


def line_total(unit_cents, quantity):
    return unit_cents * quantity
EOF
printf '# Notes\n\n' > notes.md
printf 'def shared():\n    return 0\n' > shared.py
git add -A
git commit -q -m "Start the project"
acs init --yes > "$logs/init.txt" 2>&1 || { cat "$logs/init.txt"; exit 2; }
git add -A
git commit -q -m "Adopt ACS"
calc_start=$(git hash-object calc.py)
notes_start=$(git hash-object notes.md)

tools=("Bash(git:*)" "mcp__plugin_acs_acs__*")
session a "Add a function add_tax(cents, rate_percent) at the end of calc.py that returns the \
taxed price in integer cents. Add the line 'tax: done' at the end of notes.md. Do not commit \
and do not run git. Then stop." "${tools[@]}"
check "A: the session ran with the hooks" "hooks_ran a"
check "A: both files were changed" "grep -q 'def add_tax' calc.py && grep -q 'tax: done' notes.md"
calc_a=$(git hash-object calc.py)
notes_a=$(git hash-object notes.md)

session b "Add a function round_to(cents, step) at the end of calc.py that rounds to the \
nearest multiple of step. Add the line 'rounding: done' at the end of notes.md. Do not commit \
and do not run git. Then stop." "${tools[@]}"
check "B: both files were changed" "grep -q 'def round_to' calc.py && grep -q 'rounding: done' notes.md"
check "B kept A's work" "grep -q 'def add_tax' calc.py && grep -q 'tax: done' notes.md"
calc_b=$(git hash-object calc.py)
notes_b=$(git hash-object notes.md)

ses_a=$(ses_of a)
ses_b=$(ses_of b)
echo "session A: ${ses_a:-none}   session B: ${ses_b:-none}"
check "both sessions are in acs log" "[ -n \"$ses_a\" ] && [ -n \"$ses_b\" ] && [ \"$ses_a\" != \"$ses_b\" ]"
acs log --path calc.py > "$logs/log-calc.txt" 2>&1
check "acs log --path calc.py lists both" \
    "grep -q \"${ses_a:-missing}\" '$logs/log-calc.txt' && grep -q \"${ses_b:-missing}\" '$logs/log-calc.txt'"

# blamed <text> <session>: the blamed line holding the text names the session.
blamed() {
    node -e '
        const out = JSON.parse(require("fs").readFileSync(process.argv[1], "utf8"));
        const line = (out.lines || []).map((l) => JSON.stringify(l)).find((l) => l.includes(process.argv[2]));
        process.exit(line && line.includes(process.argv[3]) ? 0 : 1);
    ' "$logs/blame.json" "$1" "$2"
}
acs blame calc.py --json > "$logs/blame.json" 2>&1
acs blame calc.py > "$logs/blame.txt" 2>&1
check "acs blame gives add_tax to A" "blamed 'def add_tax' \"${ses_a:-missing}\""
check "acs blame gives round_to to B" "blamed 'def round_to' \"${ses_b:-missing}\""
acs stats --json > "$logs/stats.json" 2>&1
check "acs stats exits 0" "[ $? -eq 0 ]"

# Undo the last session: a dry run first, then for real.
acs undo --last --json > "$logs/undo-last-dry.json" 2>&1
check "undo --last (dry run) exits 0" "[ $? -eq 0 ]"
check "undo --last names B" "grep -q \"${ses_b:-missing}\" '$logs/undo-last-dry.json'"
check "the dry run changed nothing" \
    "[ \"\$(git hash-object calc.py)\" = \"$calc_b\" ] && [ \"\$(git hash-object notes.md)\" = \"$notes_b\" ]"
acs undo --last --apply > "$logs/undo-last.txt" 2>&1
check "undo --last --apply exits 0" "[ $? -eq 0 ]"
cat "$logs/undo-last.txt"
check "B's work is gone" "! grep -q 'def round_to' calc.py && ! grep -q 'rounding: done' notes.md"
check "A's work is byte-identical" \
    "[ \"\$(git hash-object calc.py)\" = \"$calc_a\" ] && [ \"\$(git hash-object notes.md)\" = \"$notes_a\" ]"

# The person commits A's work.
git add calc.py notes.md
git commit -q -m "Add tax" > "$logs/commit-tax.txt" 2>&1
check "the person's commit of A's work goes through" "[ $? -eq 0 ]"
tax_commit=$(git rev-parse HEAD)
check "it carries A's trailer" "git log -1 --format=%B | grep -q \"ACS-Session: ${ses_a:-missing}\""
acs sync > "$logs/sync.txt" 2>&1
acs verify > "$logs/verify.txt" 2>&1
check "acs verify exits 0" "[ $? -eq 0 ]"

# Undo a session whose work is committed: a new commit, no rewrite.
acs undo "${ses_a:-missing}" --apply > "$logs/undo-a.txt" 2>&1
check "undo of the committed session exits 0" "[ $? -eq 0 ]"
cat "$logs/undo-a.txt"
check "A's work is gone from the files" \
    "[ \"\$(git hash-object calc.py)\" = \"$calc_start\" ] && [ \"\$(git hash-object notes.md)\" = \"$notes_start\" ]"
check "it was undone with a new commit on top" \
    "[ \"\$(git rev-parse HEAD)\" != \"$tax_commit\" ] && git merge-base --is-ancestor $tax_commit HEAD"
check "the work tree is clean" "[ -z \"\$(git status --porcelain)\" ]"

# A person's commit against a live claim. Session C edits shared.py and then waits, so its claim
# is live while the person commits.
session c "Add the line '# reviewed' at the end of shared.py. Then run the shell command \
'sleep 45' once. Then say done and stop. Do not commit." "Bash(sleep:*)" "mcp__plugin_acs_acs__*" &
held=0
for _ in $(seq 1 60); do
    if acs status --json 2> /dev/null | grep -q 'shared.py'; then held=1; break; fi
    sleep 2
done
acs status > "$logs/status-during-c.txt" 2>&1
check "session C holds shared.py" "[ $held -eq 1 ]"
head_before=$(git rev-parse HEAD)
git add shared.py
git commit -m "Person commits a claimed file" > "$logs/commit-rejected.txt" 2>&1
commit_status=$?
cat "$logs/commit-rejected.txt"
check "the person's commit is rejected" "[ $commit_status -ne 0 ] && [ \"\$(git rev-parse HEAD)\" = \"$head_before\" ]"
check "the rejection says who holds the file" "grep -q 'claimed by' '$logs/commit-rejected.txt'"
git restore --staged shared.py 2> /dev/null
wait
check "C: the session ran with the hooks" "hooks_ran c"
finish
