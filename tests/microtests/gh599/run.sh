#!/bin/sh
# Replayable micro-test for the fleet-epic-split rewrite (issue #599).
#
# N=1 run per arm, DRY RUN ONLY (the prompt forbids creating/editing/commenting
# on issues): the model must stop at the confirmation-table step and print the
# table plus every would-be issue body. Old arm embeds fleet-epic-split from
# origin/main (the 38-line pre-absorption text); new arm embeds this branch's
# HEAD text (four inputs, dedupe, confirmation table, cross-refs, provenance).
# Prompts are identical except the embedded skill text.
#
# The new skill points at .claude/skills/issue-intake/templates.md for the
# single body contract (ready-bar + Wiring audit slot + Predicted surface).
# The run happens inside the repo worktree, so that pointer must resolve for
# the new arm to pass scoring — the pointer wiring is part of what is tested.
#
# Usage: sh tests/microtests/gh599/run.sh
set -eu

DIR=$(cd "$(dirname "$0")" && pwd)
TS=$(date +%Y%m%d-%H%M%S)

pi -p --model z-ai/glm-5.3-flash --exclude-tools edit,write \
  --session-id "microtest-gh599-old-${TS}" \
  "$(cat "$DIR/prompt-old.md")" > "$DIR/out-old-1.md" 2>&1
echo "run old done (microtest-gh599-old-${TS}) -> out-old-1.md"

pi -p --model z-ai/glm-5.3-flash --exclude-tools edit,write \
  --session-id "microtest-gh599-new-${TS}" \
  "$(cat "$DIR/prompt-new.md")" > "$DIR/out-new-1.md" 2>&1
echo "run new done (microtest-gh599-new-${TS}) -> out-new-1.md"

echo "micro-test complete: ts ${TS}, N=1 per arm (dry run)"
