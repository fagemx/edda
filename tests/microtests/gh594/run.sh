#!/bin/sh
# Replayable micro-test for the wiring verdict slot (issue #594, PR #629).
#
# Runs the control (fleet-review text from origin/main) and the variant
# (this branch's fleet-review text) against the same fake diff, each with a
# cheap model, distinct timestamped session ids, and full outputs saved.
#
# Usage: sh tests/microtests/gh594/run.sh
set -eu

DIR=$(cd "$(dirname "$0")" && pwd)
TS=$(date +%Y%m%d-%H%M%S)

pi -p --model z-ai/glm-5.3-flash --exclude-tools edit,write \
  --session-id "microtest-control-${TS}" \
  "$(cat "$DIR/prompt-control.md")" > "$DIR/out-control.md" 2>&1

pi -p --model z-ai/glm-5.3-flash --exclude-tools edit,write \
  --session-id "microtest-variant-${TS}" \
  "$(cat "$DIR/prompt-variant.md")" > "$DIR/out-variant.md" 2>&1

echo "wrote $DIR/out-control.md and $DIR/out-variant.md (session ts ${TS})"
