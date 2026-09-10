#!/bin/sh
# verdict-drift.sh — one-line adapter (GH-1105; mechanism.shell-role=
# one-line-adapter-only). The drift query itself is the product verb
# `edda review drift` (crates/edda-cli/src/cmd_review/drift.rs), which
# reads the same §7 verdict comments through the same author-trust rule
# as `edda review deliver` (GH-1103). Output lines and exit codes are
# byte-compatible with the shell this replaces (0 clean, 1 not ready,
# 2 cannot judge), so daily-digest.sh and every other caller keep
# working unchanged. EDDA_REPO and EDDA_OPEN_PR_LIMIT still apply — the
# verb reads both. Read-only.
exec "${EDDA_BIN:-edda}" review drift "$@"
