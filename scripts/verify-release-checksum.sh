#!/usr/bin/env bash
set -euo pipefail

if [ "$#" -ne 2 ]; then
  echo "usage: verify-release-checksum.sh <archive> <checksum>" >&2
  exit 2
fi

archive="$1"
checksum="$2"

if [ ! -f "$archive" ] || [ ! -f "$checksum" ]; then
  echo "::error::Missing release archive or checksum: $archive / $checksum" >&2
  exit 1
fi

actual_hash="$(sha256sum "$archive" | awk '{ print $1 }')"

if grep -q $'\r' "$checksum"; then
  echo "::error::Checksum sidecar contains CRLF: $checksum" >&2
  exit 1
fi

# Compare bytes directly. Command substitution cannot be used here because Bash
# discards embedded NUL bytes before a string comparison.
if ! printf '%s  %s\n' "$actual_hash" "$archive" | cmp -s - "$checksum"; then
  echo "::error::Checksum sidecar must be one exact ASCII LF line: $checksum" >&2
  exit 1
fi
