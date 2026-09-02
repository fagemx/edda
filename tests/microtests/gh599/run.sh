#!/bin/sh
# Replayable micro-test for the fleet-epic-split rewrite (issue #599, PR #653
# review round 1 fix). Redesigned to remove the round-1 confounds:
#
#   (a) both arms run with an EMPTY temp directory (`mktemp -d`) as cwd.
#       Isolation is by prompt and cwd only, not a sandbox: the model keeps
#       its read/bash tools and OLDPWD still points at the repository. What
#       was observed in the committed N=1 run: the old arm did not import
#       repository conventions;
#   (b) the shared prompt frame is byte-identical for both arms and adds no
#       requirement of its own (no confirmation-table demand, no body-shape
#       demand) — it only gives the dry-run/isolation rule;
#   (c) prompts embed the input (input-stage2.md = epic #560 Stage 2) and the
#       skill text — old = origin/main blob, new = HEAD blob;
#   (d) N=1 run per arm, dry run only (no issue is created).
#
# Usage: sh tests/microtests/gh599/run.sh   (from anywhere; needs `pi`, `git`)
set -eu

DIR=$(cd "$(dirname "$0")" && pwd)
TS=$(date +%Y%m%d-%H%M%S)
WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' 0 HUP INT TERM

cd "$WORK"

pi -p --model z-ai/glm-5.3-flash --exclude-tools edit,write \
  --session-id "microtest-gh599-old-${TS}" \
  "$(cat "$DIR/prompt-old.md")" > "$DIR/out-old-1.md" 2>&1
echo "run old done (microtest-gh599-old-${TS}, cwd=${WORK}) -> out-old-1.md"

pi -p --model z-ai/glm-5.3-flash --exclude-tools edit,write \
  --session-id "microtest-gh599-new-${TS}" \
  "$(cat "$DIR/prompt-new.md")" > "$DIR/out-new-1.md" 2>&1
echo "run new done (microtest-gh599-new-${TS}, cwd=${WORK}) -> out-new-1.md"

echo "micro-test complete: ts ${TS}, N=1 per arm (dry run, isolated temp cwd)"
