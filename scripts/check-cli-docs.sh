#!/usr/bin/env bash
# check-cli-docs.sh — machine-detectable coverage check for docs/reference/cli.md (GH-650).
#
# Every verb the built `edda` binary exposes must be documented in
# docs/reference/cli.md, either as a `### edda <verb>` section or as a row in
# the "Internal / experimental" table. Without this check the doc silently
# falls behind the CLI surface (it stopped at 0.2 while the binary reached 0.4).
#
# Exit codes:
#   0 — every verb is documented
#   1 — at least one verb is undocumented, or the documented version does not
#       match the binary (each problem printed on stderr)
#   2 — the edda binary could not be found (build it, install it on PATH, or
#       set EDDA_BIN)
#
# EDDA_BIN is honoured so CI can point at a freshly built binary:
#   EDDA_BIN=target/debug/edda bash scripts/check-cli-docs.sh
set -u

DOC="docs/reference/cli.md"

# Binary resolution order: explicit EDDA_BIN override, then target/debug/edda
# (a local build), then `edda` on PATH. CI always sets EDDA_BIN to the freshly
# built binary; the PATH fallback keeps the bare `bash scripts/check-cli-docs.sh`
# working on machines with an installed edda but no local build. The version
# check below guards against a PATH binary older than the documented surface.
if [ -n "${EDDA_BIN:-}" ]; then
  if ! command -v "$EDDA_BIN" >/dev/null 2>&1; then
    echo "error: EDDA_BIN points at a non-existent or non-executable binary: '$EDDA_BIN'" >&2
    exit 2
  fi
elif [ -x "target/debug/edda" ]; then
  EDDA_BIN="target/debug/edda"
elif command -v edda >/dev/null 2>&1; then
  EDDA_BIN="edda"
else
  echo "error: edda binary not found" >&2
  echo "       build it first (cargo build -p edda), install edda on PATH, or point EDDA_BIN at an existing binary." >&2
  exit 2
fi

if [ ! -f "$DOC" ]; then
  echo "error: doc not found: $DOC" >&2
  exit 1
fi

# Verbs the binary actually exposes (from the Commands: block of --help).
# The sed range includes the `Commands:` / `Options:` delimiter lines and the
# entries are indented; keep only real entry lines (two-space indent, no colon).
verbs=$("$EDDA_BIN" --help | sed -n '/^Commands:/,/^Options:/p' \
  | awk '/^  [a-z]/ && $1 != "help" { print $1 }')

if [ -z "$verbs" ]; then
  echo "error: could not parse the command list from '$EDDA_BIN --help'" >&2
  exit 2
fi

# Documented as full sections: `### edda <verb>` (headings in the doc are
# written with backticks, e.g. `### `edda claim`` — accept both forms).
sections=$(sed -nE 's/^#[#]+[[:space:]]*`?edda[[:space:]]+`?([a-z][a-z-]*)`?[[:space:]]*$/\1/p' "$DOC")

# Documented as rows of the "Internal / experimental" table (`| \`verb\` | ... |`).
internal=$(awk '
  /[Ii]nternal \/ [Ee]xperimental/ { insec = 1; next }
  insec && /^#{1,3} / && !/[Ii]nternal \/ [Ee]xperimental/ { insec = 0 }
  insec && /^\|[[:space:]]*`[a-z][a-z-]*`/ {
    line = $0
    sub(/^\|[[:space:]]*`/, "", line)
    sub(/`.*/, "", line)
    print line
  }
' "$DOC")

# The doc must state the version it was derived from, matching the binary.
# Doc convention: a line `> Documented for edda <major.minor>` near the top.
fail=0
missing=""
bin_version=$("$EDDA_BIN" --version | awk '{ print $2 }')
bin_mm=${bin_version%.*}
if ! grep -qE "^> Documented for edda ${bin_mm//./\\.}\b" "$DOC"; then
  echo "error: $DOC does not state its documented version (expected 'Documented for edda $bin_mm' to match binary $bin_version)" >&2
  fail=1
fi

for verb in $verbs; do
  if grep -qx "$verb" <<<"$sections" || grep -qx "$verb" <<<"$internal"; then
    :
  else
    missing="$missing $verb"
    echo "error: undocumented verb: $verb" >&2
    fail=1
  fi
done

if [ "$fail" -ne 0 ]; then
  if [ -n "$missing" ]; then
    echo "check-cli-docs: FAIL —$missing" >&2
  else
    echo "check-cli-docs: FAIL" >&2
  fi
  exit 1
fi

echo "check-cli-docs: OK — all $(printf '%s\n' "$verbs" | wc -l) verbs documented in $DOC"
