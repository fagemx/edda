#!/usr/bin/env bash
# Offline Windows product-adapter regression for GH-652 R4.
set -euo pipefail

root=$(CDPATH= cd -- "$(dirname "$0")/.." && pwd)
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
scratch="$tmp/scratch"
bin="$tmp/bin"
mkdir -p "$scratch" "$bin"

cat >"$bin/gh" <<'EOF'
#!/bin/sh
case "$1 $2" in
  'pr view') case "$*" in
    *headRefOid*) echo aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa ;;
    *headRefName*) echo fixture/head ;;
    *baseRefName*) echo main ;;
    *baseRefOid*) echo bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb ;;
    *title*) echo fixture ;;
    *closingIssuesReferences*) echo 650 ;;
    *body*) echo 'Issue: #650' ;;
  esac ;;
  'pr diff') echo scripts/review-pr.sh ;;
  'issue view') printf '## doneWhen\n- fixture\n' ;;
esac
EOF
cat >"$bin/uname" <<'EOF'
#!/bin/sh
echo MINGW64_NT-10.0
EOF
cat >"$bin/edda" <<'EOF'
#!/bin/sh
if [ "$1 $2" = 'review --help' ]; then
  echo '--pr N --agent AGENT --model MODEL --json --resume'
  exit 0
fi
exit 2
EOF
cat >"$bin/claude" <<'EOF'
#!/bin/sh
test "$*" = '--help' || exit 99
echo '--tools <tools> --disallowedTools <tools> --permission-mode <mode>'
EOF
cat >"$bin/edda.cmd" <<'EOF'
@echo off
if "%1"=="review" if "%2"=="--help" (
 echo --pr N --agent AGENT --model MODEL --json --resume
 exit /b 0
)
if "%FIXTURE_DISQUALIFIED%"=="1" (
 echo {"subject":{"head_sha":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","subject_seen":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","worktree_check":"unchanged"},"reviewer":{"tool_policy":"hard","model_requested":"fixture-model","model_observed":"fixture-model","session_id":"fixture-session"},"verdict":"lgtm","qualified":true,"disqualifiers":["gate-red"],"cost":{"usd":null}}
 exit /b 3
)
echo {"subject":{"head_sha":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","subject_seen":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","worktree_check":"unchanged"},"reviewer":{"tool_policy":"hard","model_requested":"fixture-model","model_observed":"fixture-model","session_id":"fixture-session"},"verdict":"lgtm","qualified":true,"cost":{"usd":null}}
exit /b 0
EOF
chmod +x "$bin/gh" "$bin/uname" "$bin/claude" "$bin/edda"
export PATH="$bin:/c/Program Files/Git/usr/bin:/c/Program Files/Git/bin:$PATH"
export EDDA_FLEET_SCRATCH="$scratch" EDDA_FLEET_ROOT="$root" EDDA_REVIEW_PRODUCT_ADAPTER=1
windows_path="$(cygpath -w "$bin");$(cygpath -wp "$PATH")"
pwsh_bin=$(command -v pwsh)

"$root/scripts/review-pr.sh" 9998 1 --dry-run >/dev/null
mkdir -p "$scratch/wt-review-pr9998"
env PATH="$windows_path" "$pwsh_bin" -NoProfile -NonInteractive -ExecutionPolicy Bypass -File "$scratch/review-pr9998-r1-lane.ps1"
grep -qx 'DISPATCH_EXIT=0' "$scratch/review-pr9998-r1.done"
grep -qx 'QUALIFIED=true' "$scratch/review-pr9998-r1.done"
grep -qx 'DISQUALIFIERS=' "$scratch/review-pr9998-r1.done"
grep -qx 'LGTM, P0=0, P1=0' "$scratch/review-pr9998-r1.log"

export FIXTURE_DISQUALIFIED=1
"$root/scripts/review-pr.sh" 9998 2 --dry-run >/dev/null
env PATH="$windows_path" "$pwsh_bin" -NoProfile -NonInteractive -ExecutionPolicy Bypass -File "$scratch/review-pr9998-r2-lane.ps1" || [ "$?" = 3 ]
grep -qx 'DISPATCH_EXIT=3' "$scratch/review-pr9998-r2.done"
grep -qx 'QUALIFIED=false' "$scratch/review-pr9998-r2.done"
grep -qx 'DISQUALIFIERS=gate-red' "$scratch/review-pr9998-r2.done"
! grep -q '^<<<VERDICT' "$scratch/review-pr9998-r2.log"
echo 'review product adapter R4 fixture passed'
