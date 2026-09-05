#!/usr/bin/env bash
# ledger-sync.sh — GH-671 committed-mirror trigger (4090).
#
# Exports the machine-local ledger to the git-tracked mirror at docs/ledger/
# and commits only that directory, so decisions ride the next push. The mirror
# is what `edda sync --from-mirror docs/ledger` imports on another machine;
# generated files under docs/ledger must never be hand-edited.
#
# Trigger choice (recorded decision): fleet.ledger-sync-trigger=scheduled-on-4090
# — Windows Task Scheduler invokes this script periodically. This script never
# registers the production task itself; tests exercise it only against throwaway
# repositories with a stubbed `edda`.
#
# Usage: scripts/fleet/ledger-sync.sh   (run from anywhere inside the repo)
set -euo pipefail

if ! command -v edda >/dev/null 2>&1; then
    echo "ledger-sync: edda not on PATH — cannot export the ledger" >&2
    exit 1
fi

repo_root=$(git rev-parse --show-toplevel)
mirror="$repo_root/docs/ledger"

# EDDA_MACHINE (when set) names the exporting machine in INDEX.md; otherwise
# edda resolves the identity itself (GH-806).
# shellcheck disable=SC2086
edda export md --out "$mirror" ${EDDA_MACHINE:+--machine "$EDDA_MACHINE"}

# Commit only the mirror: a path-limited commit never sweeps unrelated WIP
# (staged files, in-progress edits elsewhere in the tree stay untouched).
if [ -n "$(git status --porcelain -- docs/ledger)" ]; then
    git add -- docs/ledger
    git commit -m "chore(ledger): refresh committed mirror" -- docs/ledger
    echo "ledger-sync: mirror committed — push when ready"
else
    echo "ledger-sync: mirror unchanged"
fi
