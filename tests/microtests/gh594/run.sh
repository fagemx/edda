#!/bin/sh
# Replayable micro-test for the wiring verdict slot (issue #594, PR #629).
#
# N=3 runs per arm against the same fake diff (defects modeled on the real
# 5abbfb7 miss: a with_model setter with a test caller whose field never
# reaches the spawn path; a debug-logged swallowed write on a receipt path).
# Control embeds fleet-review from origin/main; variant embeds this branch's
# fleet-review (with the Wiring verdict slot). Prompts are identical except
# the skill text. Outputs are committed per run for scoring.
#
# Usage: sh tests/microtests/gh594/run.sh
set -eu

DIR=$(cd "$(dirname "$0")" && pwd)
TS=$(date +%Y%m%d-%H%M%S)
N=3

i=1
while [ "$i" -le "$N" ]; do
  pi -p --model z-ai/glm-5.3-flash --exclude-tools edit,write \
    --session-id "microtest-control-${TS}-${i}" \
    "$(cat "$DIR/prompt-control.md")" > "$DIR/out-control-${i}.md" 2>&1
  echo "run control-${i} done (microtest-control-${TS}-${i}) -> out-control-${i}.md"
  i=$((i + 1))
done

i=1
while [ "$i" -le "$N" ]; do
  pi -p --model z-ai/glm-5.3-flash --exclude-tools edit,write \
    --session-id "microtest-variant-${TS}-${i}" \
    "$(cat "$DIR/prompt-variant.md")" > "$DIR/out-variant-${i}.md" 2>&1
  echo "run variant-${i} done (microtest-variant-${TS}-${i}) -> out-variant-${i}.md"
  i=$((i + 1))
done

echo "micro-test complete: ts ${TS}, N=${N} per arm"
