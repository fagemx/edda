#!/bin/sh
# GH-702: execute the generated POSIX runner against an old/fake backend.
# An overprivileged launch actually alters the canary; help probing is free.
set -eu
root=$(CDPATH= cd -- "$(dirname "$0")/.." && pwd)
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' 0 HUP INT TERM
mkdir -p "$tmp/bin" "$tmp/scratch/wt-review-pr9998"
git -C "$tmp/scratch/wt-review-pr9998" init -q
git -C "$tmp/scratch/wt-review-pr9998" config user.name fixture
git -C "$tmp/scratch/wt-review-pr9998" config user.email fixture@example.invalid
printf 'clean\n' > "$tmp/scratch/wt-review-pr9998/tracked.txt"
printf 'ignored.txt\n' > "$tmp/scratch/wt-review-pr9998/.gitignore"
git -C "$tmp/scratch/wt-review-pr9998" add tracked.txt .gitignore
git -C "$tmp/scratch/wt-review-pr9998" commit -qm fixture
export GH_HEAD=$(git -C "$tmp/scratch/wt-review-pr9998" rev-parse HEAD)
export EDDA_FLEET_SCRATCH="$tmp/scratch" EDDA_REVIEW_SPEC="$root/REVIEW.md"
export EDDA_FLEET_ROOT="$root" CANARY="$tmp/canary" CALLS="$tmp/calls"
export REVIEW_TREE="$tmp/scratch/wt-review-pr9998"
cat > "$tmp/bin/gh" <<'EOF'
#!/bin/sh
case "$*" in
 *headRefOid*|*baseRefOid*) echo "$GH_HEAD" ;;
 *headRefName*|*baseRefName*) echo main ;;
 *title*) echo fixture ;;
 *body*) printf 'Issue: #702\n## doneWhen\nRestrict reviewers\n' ;;
 *--name-only*) echo scripts/review-pr.sh ;;
esac
EOF
cat > "$tmp/bin/uname" <<'EOF'
#!/bin/sh
if [ "${TEST_WIN:-0}" = 1 ]; then echo MINGW64; else echo Linux; fi
EOF
cat > "$tmp/bin/edda" <<'EOF'
#!/bin/sh
if [ "$*" = 'dispatch --help' ]; then
 [ "$BACKEND" = old ] || echo '--tools <TOOLS> --exclude-tools <EXCLUDE_TOOLS> --permission-mode <MODE>'
 exit 0
fi
echo launch >> "$CALLS"
case "$*" in
 *"--permission-mode plan"*"--tools Read,Grep,Glob,Bash"*"--exclude-tools Edit,Write,NotebookEdit,mcp__*"*) : ;;
 *) echo TAMPERED > "$CANARY" ;;
esac
if [ "$BACKEND" = mutate ]; then
  printf 'changed-again\n' > "$REVIEW_TREE/tracked.txt"
  printf 'changed-untracked\n' > "$REVIEW_TREE/untracked.txt"
  printf 'changed-ignored\n' > "$REVIEW_TREE/ignored.txt"
fi
EOF
cat > "$tmp/bin/claude" <<'EOF'
#!/bin/sh
if [ "$*" = '--help' ]; then
 [ "$BACKEND" = old-claude ] || echo '--tools <tools> --disallowedTools, --disallowed-tools <tools> --permission-mode <mode>'
 exit 0
fi
echo launch >> "$CALLS"
case "$*" in
 *"--permission-mode plan"*"--tools Read,Grep,Glob,Bash"*"--disallowedTools Edit,Write,NotebookEdit,mcp__*"*) : ;;
 *) echo TAMPERED > "$CANARY" ;;
esac
if [ "$BACKEND" = mutate ]; then
  printf 'changed-again\n' > "$REVIEW_TREE/tracked.txt"
  printf 'changed-untracked\n' > "$REVIEW_TREE/untracked.txt"
  printf 'changed-ignored\n' > "$REVIEW_TREE/ignored.txt"
fi
printf '{"result":"fixture","session_id":"fixture"}\n'
EOF
chmod +x "$tmp/bin/"*
export PATH="$tmp/bin:$PATH"
sh "$root/scripts/review-pr.sh" 9998 --dry-run > "$tmp/generated"
runner="$tmp/scratch/review-pr9998-r1-run.sh"
# Suppress cleanup only: the fixture cwd is not a registered worktree.
sed '/git -C .* worktree remove /d' "$runner" > "$tmp/run.sh"

# F4 baseline: this intentionally removes the new preflight and capability
# flags from the historical unguarded dispatch call shape. It is a bounded
# surrogate, not a claim that an old Claude binary was available; the separate
# real-Claude receipt records the installed 2.1.259 before/after experiment.
sed \
  -e 's/if review_capabilities [a-z-]* > .*; then/if true; then/' \
  -e "s/ --permission-mode 'plan'//" \
  -e "s/ --tools 'Read,Grep,Glob,Bash'//" \
  -e "s/ --exclude-tools 'Edit,Write,NotebookEdit,mcp__\*'//" \
  -e "s/ --disallowedTools 'Edit,Write,NotebookEdit,mcp__\*'//" \
  "$tmp/run.sh" > "$tmp/unguarded-old-run.sh"
BACKEND=unguarded; export BACKEND
printf ORIGINAL > "$CANARY"
: > "$CALLS"
printf brief > "$tmp/scratch/review-pr9998-r1-brief.md"
failures=0
sh "$tmp/unguarded-old-run.sh" > "$tmp/old-out" 2>&1 || old_rc=$?
old_rc=${old_rc:-0}
# This control asks one thing: does an UNGUARDED dispatch reach the backend?
# So it reads DISPATCH_EXIT from the receipt, not the runner's process exit.
# Since GH-867 the process exit also folds in worktree and task teardown, and
# this fixture's wt-review-pr9998 is a standalone `git init` repo rather than a
# worktree registered against $ROOT, so `git worktree remove` fails here by
# construction and FINAL_EXIT is always 2 — which made the control look dead
# when it was working (GH-928).
unguarded_dispatch_exit=$(sed -n 's/^DISPATCH_EXIT=//p' "$tmp/scratch/review-pr9998-r1.done" | head -1)
if [ "$(cat "$CANARY")" != TAMPERED ] || [ ! -s "$CALLS" ] || [ "$unguarded_dispatch_exit" != 0 ]; then
  cat "$tmp/old-out" >&2 || true
  echo "FAIL unguarded baseline: unguarded dispatch did not reach the backend (canary=$(cat "$CANARY") DISPATCH_EXIT=${unguarded_dispatch_exit:-absent} process_exit=$old_rc)" >&2
  failures=$((failures + 1))
fi
for BACKEND in old modern old-claude fallback mutate; do
 export BACKEND
 printf ORIGINAL > "$CANARY"
 : > "$CALLS"
 # Start with every source category represented. tracked.txt is already dirty,
 # then mutate changes it again without changing porcelain status; untracked
 # and ignored files exercise the other two snapshot scopes.
 printf 'dirty-before-review\n' > "$REVIEW_TREE/tracked.txt"
 printf 'untracked-before-review\n' > "$REVIEW_TREE/untracked.txt"
 printf 'ignored-before-review\n' > "$REVIEW_TREE/ignored.txt"
 if [ "$BACKEND" = old-claude ] || [ "$BACKEND" = fallback ]; then
   head -c 31000 /dev/zero > "$tmp/scratch/review-pr9998-r1-brief.md"
 else
   printf brief > "$tmp/scratch/review-pr9998-r1-brief.md"
 fi
 rc=0
 sh "$tmp/run.sh" > "$tmp/out" 2>&1 || rc=$?
 if [ "$(cat "$CANARY")" != ORIGINAL ]; then
   echo "FAIL $BACKEND: backend wrote canary"; failures=$((failures + 1))
 fi
 case "$BACKEND" in
 old|old-claude)
   if [ "$rc" -eq 0 ] || [ -s "$CALLS" ]; then
     echo "FAIL $BACKEND: capability refusal did not precede launch"; failures=$((failures + 1))
   fi ;;
 mutate)
   if [ ! -s "$CALLS" ] || [ "$rc" -eq 0 ] || ! grep -q '^WORKTREE_CHECK=failed' "$tmp/scratch/review-pr9998-r1.done"; then
     cat "$tmp/out" >&2 || true
     echo 'FAIL mutate: tracked, untracked, or ignored source change was accepted' >&2; failures=$((failures + 1))
   fi ;;
 *)
   # Same reading as the unguarded control above: "did the dispatch run" is
   # DISPATCH_EXIT, not the process exit, which since GH-867 also carries
   # teardown the fixture cannot satisfy (GH-928).
   dispatch_exit=$(sed -n 's/^DISPATCH_EXIT=//p' "$tmp/scratch/review-pr9998-r1.done" | head -1)
   if [ ! -s "$CALLS" ] || [ "$dispatch_exit" != 0 ]; then
     echo "FAIL $BACKEND: capable backend never launched (DISPATCH_EXIT=${dispatch_exit:-absent} process_exit=$rc)"; failures=$((failures + 1))
   fi
   if ! grep -q '^TOOL_FLAGS=.*--permission-mode.*mcp__' "$tmp/scratch/review-pr9998-r1.done"; then
     echo "FAIL $BACKEND: no actual flags receipt"; failures=$((failures + 1))
   fi ;;
 esac
done
# No early exit: the Windows block below must run even when a POSIX case
# failed, so the operator sees every failing case in one run (GH-928). The
# single gate is at the end of the file.
if [ "$failures" -eq 0 ]; then
  echo 'review capability canaries passed (unguarded baseline; old/modern dispatch and fallback; all source scopes)'
fi

# On Windows execute the generated PowerShell lane too. Functions stand in
# for the two binaries; any non-help call with missing flags writes canary.
if command -v pwsh >/dev/null 2>&1 && command -v cygpath >/dev/null 2>&1; then
  TEST_WIN=1 sh "$root/scripts/review-pr.sh" 9998 --dry-run > "$tmp/generated-win"
  cat > "$tmp/driver.ps1" <<'EOF'
param([string]$Lane)
function edda {
  if (($args -join ' ') -eq 'dispatch --help') {
    if ($env:BACKEND -ne 'old') { '--tools <TOOLS> --exclude-tools <TOOLS> --permission-mode <MODE>' }
    $global:LASTEXITCODE = 0; return
  }
  Add-Content $env:CALLS launch
  if ($args -notcontains $expectedTools -or $args -notcontains 'Edit,Write,NotebookEdit,mcp__*' -or $args -notcontains 'plan') { Set-Content $env:CANARY TAMPERED }
  if ($env:BACKEND -eq 'mutate') {
    Set-Content (Join-Path $env:REVIEW_TREE 'tracked.txt') changed-again
    Set-Content (Join-Path $env:REVIEW_TREE 'untracked.txt') changed-untracked
    Set-Content (Join-Path $env:REVIEW_TREE 'ignored.txt') changed-ignored
  }
  $global:LASTEXITCODE = 0
}
function claude {
  if (($args -join ' ') -eq '--help') {
    if ($env:BACKEND -ne 'old-claude') { '--tools <tools> --disallowedTools, --disallowed-tools <tools> --permission-mode <mode>' }
    $global:LASTEXITCODE = 0; return
  }
  Add-Content $env:CALLS launch
  if ($args -notcontains $expectedTools -or $args -notcontains 'Edit,Write,NotebookEdit,mcp__*' -or $args -notcontains 'plan') { Set-Content $env:CANARY TAMPERED }
  if ($env:BACKEND -eq 'mutate') {
    Set-Content (Join-Path $env:REVIEW_TREE 'tracked.txt') changed-again
    Set-Content (Join-Path $env:REVIEW_TREE 'untracked.txt') changed-untracked
    Set-Content (Join-Path $env:REVIEW_TREE 'ignored.txt') changed-ignored
  }
  '{"result":"fixture","session_id":"fixture","modelUsage":{"fixture":{}}}'
  $global:LASTEXITCODE = 0
}
$expectedTools = 'Read,Grep,Glob,Bash'
& $Lane
exit $LASTEXITCODE
EOF
  CANARY=$(cygpath -w "$CANARY"); export CANARY
  CALLS=$(cygpath -w "$CALLS"); export CALLS
  REVIEW_TREE=$(cygpath -w "$REVIEW_TREE"); export REVIEW_TREE
  for BACKEND in old modern old-claude fallback mutate; do
    export BACKEND
    printf ORIGINAL > "$CANARY"
    : > "$CALLS"
    printf 'dirty-before-review\n' > "$tmp/scratch/wt-review-pr9998/tracked.txt"
    printf 'untracked-before-review\n' > "$tmp/scratch/wt-review-pr9998/untracked.txt"
    printf 'ignored-before-review\n' > "$tmp/scratch/wt-review-pr9998/ignored.txt"
    if [ "$BACKEND" = old-claude ] || [ "$BACKEND" = fallback ]; then
      head -c 31000 /dev/zero > "$tmp/scratch/review-pr9998-r1-brief.md"
    else
      echo brief > "$tmp/scratch/review-pr9998-r1-brief.md"
    fi
    rc=0
    pwsh -NoProfile -File "$(cygpath -w "$tmp/driver.ps1")" \
      -Lane "$(cygpath -w "$tmp/scratch/review-pr9998-r1-lane.ps1")" > "$tmp/win-out" 2>&1 || rc=$?
    if [ "$(tr -d '\r\n' < "$CANARY")" != ORIGINAL ]; then
      echo "FAIL Windows $BACKEND: canary changed"; failures=$((failures + 1))
    fi
    case "$BACKEND" in
      old|old-claude)
        if [ "$rc" -eq 0 ] || [ -s "$CALLS" ]; then cat "$tmp/win-out"; echo "FAIL Windows $BACKEND: not refused"; failures=$((failures + 1)); fi ;;
      mutate)
        if [ ! -s "$CALLS" ] || [ "$rc" -eq 0 ] || ! grep -q '^WORKTREE_CHECK=failed' "$tmp/scratch/review-pr9998-r1.done"; then cat "$tmp/win-out"; echo 'FAIL Windows mutate: source change was accepted'; failures=$((failures + 1)); fi ;;
      *)
        win_dispatch_exit=$(sed -n 's/^DISPATCH_EXIT=//p' "$tmp/scratch/review-pr9998-r1.done" | head -1)
        if [ ! -s "$CALLS" ] || [ "$win_dispatch_exit" != 0 ]; then cat "$tmp/win-out"; echo "FAIL Windows $BACKEND: no successful launch (DISPATCH_EXIT=${win_dispatch_exit:-absent} process_exit=$rc)"; failures=$((failures + 1)); fi
        grep -q '^TOOL_FLAGS=.*--permission-mode.*mcp__' "$tmp/scratch/review-pr9998-r1.done" || { echo "FAIL Windows $BACKEND: no actual flags receipt"; failures=$((failures + 1)); } ;;
    esac
  done
  if [ "$failures" -eq 0 ]; then
    echo 'Windows generated lane canaries passed (old/modern transport and source-snapshot cases)'
  fi
fi

# The one gate, after every case has had its say.
[ "$failures" -eq 0 ] || exit 1
