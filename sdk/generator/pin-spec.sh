#!/usr/bin/env bash
# Pin the event spec repo (GH-608) at a controller-approved commit and copy
# the canonical schemas + golden fixtures into sdk/spec-pin/ for the SDK
# generators and contract tests.
#
# Usage:
#   EDDA_SPEC_REPO=<url-or-path> ./pin-spec.sh <full-sha>
#
# The pin is recorded in sdk/generator/SPEC_PIN.env. Do NOT run this against
# a moving ref — the controller hands off the pinned SHA (SDK_HANDOFF.md).
set -euo pipefail

SHA="${1:?usage: pin-spec.sh <full-sha>}"
REPO="${EDDA_SPEC_REPO:-https://github.com/fagemx/edda.git}"
HERE="$(cd "$(dirname "$0")" && pwd)"
PIN_DIR="$HERE/../spec-pin"

case "$SHA" in
  [0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f]) ;;
  *) echo "error: pass a full 40-hex commit SHA, got '$SHA'" >&2; exit 2 ;;
esac

WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

if [ -d "$REPO/.git" ] || [ -d "$REPO/spec" ]; then
  git -C "$REPO" worktree add "$WORK/checkout" "$SHA" >/dev/null 2>&1 || {
    echo "error: local spec repo at $REPO does not contain $SHA" >&2; exit 1;
  }
else
  git clone --filter=blob:none --no-checkout "$REPO" "$WORK/clone" >/dev/null 2>&1
  git -C "$WORK/clone" checkout "$SHA" >/dev/null 2>&1 || {
    echo "error: $REPO does not contain $SHA" >&2; exit 1;
  }
  mv "$WORK/clone" "$WORK/checkout"
fi

if [ ! -f "$WORK/checkout/spec/events/registry.json" ]; then
  echo "error: $SHA does not contain spec/events/registry.json" >&2; exit 1
fi
if [ ! -f "$WORK/checkout/spec/events/canonical-v1.json" ]; then
  echo "error: $SHA does not contain spec/events/canonical-v1.json" >&2; exit 1
fi
if [ ! -d "$WORK/checkout/tests/fixtures/events" ]; then
  echo "error: $SHA does not contain tests/fixtures/events golden fixtures" >&2; exit 1
fi

rm -rf "$PIN_DIR"
mkdir -p "$PIN_DIR"
# mirror the repo layout: <pin>/spec/events and <pin>/tests/fixtures/events
cp -r "$WORK/checkout/spec" "$PIN_DIR/spec"
mkdir -p "$PIN_DIR/tests"
cp -r "$WORK/checkout/tests/fixtures" "$PIN_DIR/tests/fixtures"

cat > "$HERE/SPEC_PIN.env" <<EOF
# Pinned event spec consumed by the SDK generators and contract tests.
# Set by the controller handoff (SDK_HANDOFF.md). Do not move without
# controller approval — this is the type-source pin for the contract.
SPEC_SHA=$SHA
SPEC_REPO=$REPO
EOF

echo "pinned spec $SHA into $PIN_DIR"
