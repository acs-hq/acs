#!/bin/sh
# Checks the `acs` on PATH on a clean machine: right version, and it sets a repository up.
#
# usage: smoke.sh <release tag>
set -eu

acs --version
acs --version | grep -F "${1#v}"

repo=$(mktemp -d)
cd "$repo"
git init -q
git config user.email ci@example.com
git config user.name CI
acs init
acs init | grep -F "already set up"
acs doctor
