#!/usr/bin/env bash
# `acs uninstall` (issue 63) and the main-branch detection of `acs init` (issue 67) against the
# real tools: a clone whose main branch is `master`, a hook of the person's own, a committed
# `.claude/settings.json`, team sync with claims pushed to a bare remote, and the ACS plugin
# installed by the real `claude` CLI. No model calls: `claude plugin ...` needs no login.
#
#   tests/e2e/real/uninstall.sh <empty-scratch-dir>
#
# Needs a build of `acs` in this checkout's target/ (the newer of release and debug) and `claude`
# on PATH. Everything it writes is under the scratch directory: the repositories, ACS_HOME and
# CLAUDE_CONFIG_DIR. The marketplace is this checkout, so nothing is fetched from the network.
#
# It passes when `acs init` writes `master`, `acs doctor` agrees, and after `acs uninstall --all`
# the clone's git config, hooks, settings file and refs are byte-identical to before `acs init`,
# the remote has no `refs/acs/*` left and ACS_HOME holds nothing for the repository.

set -u

checkout=$(cd "$(dirname "$0")/../../.." && pwd)
work=$(cygpath -m "${1:?usage: uninstall.sh <empty-scratch-dir>}" 2>/dev/null || echo "$1")
exe() { if [ -e "$1/acs.exe" ]; then echo "$1/acs.exe"; else echo "$1/acs"; fi; }
release=$(exe "$checkout/target/release")
debug=$(exe "$checkout/target/debug")
if [ -x "$debug" ] && { [ ! -x "$release" ] || [ "$debug" -nt "$release" ]; }; then
    bin="$checkout/target/debug"
else
    bin="$checkout/target/release"
fi
[ -x "$(exe "$bin")" ] || {
    echo "build first: cargo build -p acs-cli" >&2
    exit 2
}
export PATH="$bin:$PATH"
export ACS_HOME="$work/acs-home"
export CLAUDE_CONFIG_DIR="$work/claude-config"
export GIT_AUTHOR_NAME=Pilot GIT_AUTHOR_EMAIL=pilot@example.com
export GIT_COMMITTER_NAME=Pilot GIT_COMMITTER_EMAIL=pilot@example.com
logs="$work/logs"
mkdir -p "$logs"

failures=0
check() { # check <name> <command...>
    local name=$1
    shift
    if "$@" >>"$logs/checks.log" 2>&1; then
        echo "PASS  $name"
    else
        echo "FAIL  $name"
        failures=$((failures + 1))
    fi
}

# A repository on master, its bare remote, and a clone (so origin/HEAD names master).
git init -q -b master "$work/seed"
(cd "$work/seed" && printf 'x\n' >a.txt && git add a.txt && git commit -q -m first)
git clone -q --bare "$work/seed" "$work/origin.git"
git clone -q "$work/origin.git" "$work/repo"
cd "$work/repo" || exit 2
printf '#!/bin/sh\necho my own hook\n' >.git/hooks/pre-commit
mkdir -p .claude
printf '{\n  "permissions": {\n    "allow": [\n      "Bash(ls)"\n    ]\n  }\n}\n' >.claude/settings.json
git add .claude && git commit -q -m settings

snap() { # snap <dir>: what uninstall must restore
    mkdir -p "$1"
    cp .git/config "$1/config"
    cp -r .git/hooks "$1/hooks"
    cp .claude/settings.json "$1/settings.json"
    git for-each-ref >"$1/refs"
    git ls-remote origin >"$1/remote"
}
snap "$work/before"

acs init >"$logs/init.txt" 2>&1
cat "$logs/init.txt"
check "init names master from origin/HEAD" grep -q "master (origin/HEAD): set as" "$logs/init.txt"
check "config.toml sets master" grep -q '^default_target_branch = "master"' .acs/policy/config.toml
check "guards.toml protects master" grep -q '^protected_branches = \["main", "release/\*", "master"\]' .acs/policy/guards.toml
acs doctor --json >"$logs/doctor.json" 2>&1
check "doctor's target_branch check passes" grep -q '"id": "target_branch"' "$logs/doctor.json"
check "doctor finds master" grep -q 'default_target_branch = \\"master\\" exists' "$logs/doctor.json"

acs features sync on >"$logs/features.txt" 2>&1
acs init >"$logs/init2.txt" 2>&1
check "the fetch refspec is added" git config --get-all remote.origin.fetch '^\+refs/acs/'
acs plugin install claude-code --marketplace "$checkout" >"$logs/plugin.txt" 2>&1
cat "$logs/plugin.txt"
cp .claude/settings.json "$logs/settings-with-plugin.json"
check "claude wrote the plugin into settings.json" grep -q 'acs@acs-plugins' .claude/settings.json
acs claim a.txt >"$logs/claim.txt" 2>&1
acs sync >"$logs/sync.txt" 2>&1
git ls-remote origin 'refs/acs/*' >"$logs/remote-acs-before.txt"
check "claims reached the remote" test -s "$logs/remote-acs-before.txt"
acs release --all >/dev/null 2>&1

acs uninstall --dry-run --all >"$logs/dry-run.txt" 2>&1
cat "$logs/dry-run.txt"
check "the dry run changed nothing" grep -q 'acs@acs-plugins' .claude/settings.json
acs uninstall --all >"$logs/uninstall.txt" 2>&1
status=$?
cat "$logs/uninstall.txt"
check "uninstall exits 0" test "$status" -eq 0
snap "$work/after"
for part in config settings.json refs remote; do
    check "$part is byte-identical" cmp "$work/before/$part" "$work/after/$part"
done
check "hooks are byte-identical" diff -r "$work/before/hooks" "$work/after/hooks"
check "no refs/acs on the remote" test -z "$(git ls-remote origin 'refs/acs/*')"
check "the policy is gone" test ! -e .acs
check "ACS_HOME holds no repository" test -z "$(ls -A "$ACS_HOME/repos" 2>/dev/null)"
check "the own hook still runs" sh .git/hooks/pre-commit

echo
if [ "$failures" -eq 0 ]; then
    echo "all checks passed (logs in $logs)"
else
    echo "$failures check(s) failed (logs in $logs)"
    exit 1
fi
