#!/bin/sh
# Prints the message that is signed for one release archive (decision 0054). The message binds the
# release tag and the archive's file name to the archive's bytes:
#
#   acs-release-v1<LF>
#   <tag><LF>
#   <archive file name><LF>
#   <lowercase hex SHA-256 of the archive><LF>
#
# Every line, the last one too, ends with a newline. `acs` builds the same text in
# crates/acs-cli/src/update/signature.rs (`message`); a test keeps the two in step, so change both
# or neither.
#
# Usage: sign-release-message.sh <tag> <archive path>
set -eu

tag=$1
archive=$2
name=$(basename "$archive")
sum=$(sha256sum "$archive" | cut -d ' ' -f 1)
printf 'acs-release-v1\n%s\n%s\n%s\n' "$tag" "$name" "$sum"
