#!/bin/sh
# Prints the Scoop manifest of a release, from acs.json.in and the checksums dist wrote.
#
# usage: render-manifest.sh <version> <directory holding the .sha256 files>
set -eu

version=$1
artifacts=$2

hash_of() {
  cut -d ' ' -f 1 "$artifacts/$1.sha256"
}

x64=$(hash_of acs-cli-x86_64-pc-windows-msvc.zip)
arm64=$(hash_of acs-cli-aarch64-pc-windows-msvc.zip)

sed -e "s/@VERSION@/$version/g" -e "s/@HASH_X64@/$x64/" -e "s/@HASH_ARM64@/$arm64/" \
  "$(dirname "$0")/acs.json.in"
