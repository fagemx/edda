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
 if [ "$BACKEND" = old ]; then exit 0; fi
 if [ "$BACKEND" = pi-old-edda ]; then
   echo '--tools <TOOLS> --exclude-tools <EXCLUDE_TOOLS> --permission-mode <MODE>'
 else
   echo '--model <MODEL> --tools <TOOLS> --exclude-tools <EXCLUDE_TOOLS> --permission-mode <MODE>'
 fi
 exit 0
fi
echo launch >> "$CALLS"
case "$*" in
 *"--agent pi"*)
   case "$*" in
     *"--model openrouter/z-ai/glm-5.3-flash"*) : ;;
     *) echo TAMPERED > "$CANARY" ;;
   esac
   case "$*" in
     *"--tools read,grep,find,ls"*) : ;;
     *) echo TAMPERED > "$CANARY" ;;
   esac
   case "$*" in
     *--exclude-tools*|*--permission-mode*|*--resume*) echo TAMPERED > "$CANARY" ;;
   esac
   ;;
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
 [ "$BACKEND" = old-claude ] || echo '--tools <tools> --disallowedTools <tools> --permission-mode <mode>'
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
cat > "$tmp/bin/pi" <<'EOF'
#!/bin/sh
if [ "$*" = '--help' ]; then
  if [ "$BACKEND" = pi-old ]; then
    echo '--model <pattern> --session-id <id>'
  else
    echo '--model <pattern> --tools, -t <tools> --exclude-tools, -xt <tools> --session-id <id>'
  fi
  exit 0
fi
echo TAMPERED > "$CANARY"
EOF
chmod +x "$tmp/bin/"*
export PATH="$tmp/bin:$PATH"

# GH-880 refusals, before any runner generation: fail-closed with one stderr
# line, exit 2, and nothing written (the stubs are never reached).
rc=0
EDDA_REVIEW_AGENT=pi EDDA_REVIEW_MODEL=claude-opus-5 sh "$root/scripts/review-pr.sh" 9998 --dry-run > "$tmp/refuse-anthropic" 2>&1 || rc=$?
if [ "$rc" -ne 2 ] || ! grep -q 'fleet.claude-subscription-transport' "$tmp/refuse-anthropic"; then
  cat "$tmp/refuse-anthropic" >&2 || true
  echo 'FAIL refusal anthropic-on-pi' >&2
  exit 1
fi
rc=0
EDDA_REVIEW_AGENT=codex sh "$root/scripts/review-pr.sh" 9998 --dry-run > "$tmp/refuse-codex" 2>&1 || rc=$?
if [ "$rc" -ne 2 ] || ! grep -q 'codex' "$tmp/refuse-codex"; then
  cat "$tmp/refuse-codex" >&2 || true
  echo 'FAIL refusal unknown-agent' >&2
  exit 1
fi

sh "$root/scripts/review-pr.sh" 9998 --dry-run > "$tmp/generated"
runner="$tmp/scratch/review-pr9998-r1-run.sh"
# Suppress cleanup only: the fixture cwd is not a registered worktree.
sed '/git -C .* worktree remove /d' "$runner" > "$tmp/run.sh"

# F4 baseline: this intentionally removes the new preflight and capability
# flags from the historical unguarded dispatch call shape. It is a bounded
# surrogate, not a claim that an old Claude binary was available; the separate
# real-Claude receipt records the installed 2.1.259 before/after experiment.
sed \
  -e '/review_capabilities /d' \
  -e "s/ --permission-mode 'plan'//" \
  -e "s/ --tools 'Read,Grep,Glob,Bash'//" \
  -e "s/ --exclude-tools 'Edit,Write,NotebookEdit,mcp__\*'//" \
  -e "s/ --disallowedTools 'Edit,Write,NotebookEdit,mcp__\*'//" \
  "$tmp/run.sh" > "$tmp/unguarded-old-run.sh"
BACKEND=unguarded; export BACKEND
printf ORIGINAL > "$CANARY"
: > "$CALLS"
printf brief > "$tmp/scratch/review-pr9998-r1-brief.md"
sh "$tmp/unguarded-old-run.sh" > "$tmp/old-out" 2>&1 || old_rc=$?
old_rc=${old_rc:-0}
if [ "$(cat "$CANARY")" != TAMPERED ] || [ ! -s "$CALLS" ] || [ "$old_rc" -ne 0 ]; then
  cat "$tmp/old-out" >&2 || true
  echo 'FAIL unguarded baseline: old dispatch shape did not write owned canary' >&2
  exit 1
fi

failures=0
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
   if [ ! -s "$CALLS" ] || [ "$rc" -ne 0 ]; then
     echo "FAIL $BACKEND: capable backend never launched"; failures=$((failures + 1))
   fi
   if ! grep -q '^TOOL_FLAGS=.*--permission-mode.*mcp__' "$tmp/scratch/review-pr9998-r1.done"; then
     echo "FAIL $BACKEND: no actual flags receipt"; failures=$((failures + 1))
   fi ;;
 esac
done
[ "$failures" -eq 0 ] || exit 1

# GH-880 pi arm: generate the pi runner and canary it. pi-modern must launch
# through edda dispatch with the exact read-only allowlist; pi-old (pi lacks
# --tools) and pi-old-edda (edda dispatch lacks --model) must be refused
# before any launch.
EDDA_REVIEW_AGENT=pi EDDA_REVIEW_MODEL=openrouter/z-ai/glm-5.3-flash sh "$root/scripts/review-pr.sh" 9998 --dry-run > "$tmp/generated-pi"
sed '/git -C .* worktree remove /d' "$tmp/scratch/review-pr9998-r1-run.sh" > "$tmp/run-pi.sh"
for BACKEND in pi-modern pi-old pi-old-edda; do
 export BACKEND
 printf ORIGINAL > "$CANARY"
 : > "$CALLS"
 printf 'dirty-before-review\n' > "$REVIEW_TREE/tracked.txt"
 printf 'untracked-before-review\n' > "$REVIEW_TREE/untracked.txt"
 printf 'ignored-before-review\n' > "$REVIEW_TREE/ignored.txt"
 printf brief > "$tmp/scratch/review-pr9998-r1-brief.md"
 rc=0
 sh "$tmp/run-pi.sh" > "$tmp/out-pi" 2>&1 || rc=$?
 if [ "$(cat "$CANARY")" != ORIGINAL ]; then
   echo "FAIL $BACKEND: backend wrote canary"; failures=$((failures + 1))
 fi
 case "$BACKEND" in
 pi-modern)
   if [ ! -s "$CALLS" ] || [ "$rc" -ne 0 ]; then
     cat "$tmp/out-pi" >&2 || true
     echo 'FAIL pi-modern: capable pi backend never launched'; failures=$((failures + 1))
   fi
   grep -q '^TRANSPORT=pi-dispatch' "$tmp/scratch/review-pr9998-r1.done" || { echo 'FAIL pi-modern: no TRANSPORT=pi-dispatch receipt'; failures=$((failures + 1)); }
   grep -qF "TOOL_FLAGS=--tools 'read,grep,find,ls'" "$tmp/scratch/review-pr9998-r1.done" || { echo 'FAIL pi-modern: TOOL_FLAGS receipt missing the allowlist'; failures=$((failures + 1)); }
   ;;
 *)
   if [ "$rc" -eq 0 ] || [ -s "$CALLS" ]; then
     cat "$tmp/out-pi" >&2 || true
     echo "FAIL $BACKEND: capability refusal did not precede launch"; failures=$((failures + 1))
   fi
   ;;
 esac
done
[ "$failures" -eq 0 ] || exit 1
echo 'review capability canaries passed (unguarded baseline; old/modern dispatch and fallback; all source scopes; pi-modern launch and pi-old/pi-old-edda refusals; anthropic/codex refusals)'

# On Windows execute the generated PowerShell lane too. Functions stand in
# for the two binaries; any non-help call with missing flags writes canary.
if command -v pwsh >/dev/null 2>&1 && command -v cygpath >/dev/null 2>&1; then
  TEST_WIN=1 sh "$root/scripts/review-pr.sh" 9998 --dry-run > "$tmp/generated-win"
  cat > "$tmp/driver.ps1" <<'EOF'
param([string]$Lane)
function edda {
  if (($args -join ' ') -eq 'dispatch --help') {
    if ($env:BACKEND -eq 'old') { $global:LASTEXITCODE = 0; return }
    if ($env:BACKEND -eq 'pi-old-edda') {
      '--tools <TOOLS> --exclude-tools <TOOLS> --permission-mode <MODE>'
    } else {
      '--model <MODEL> --tools <TOOLS> --exclude-tools <TOOLS> --permission-mode <MODE>'
    }
    $global:LASTEXITCODE = 0; return
  }
  Add-Content $env:CALLS launch
  if ($args -contains '--agent' -and $args -contains 'pi') {
    $j = ($args -join ' ')
    if ($j -notlike '*--model openrouter/z-ai/glm-5.3-flash*' -or $j -notlike '*--tools read,grep,find,ls*' -or $j -like '*--exclude-tools*' -or $j -like '*--permission-mode*' -or $j -like '*--resume*') { Set-Content $env:CANARY TAMPERED }
    $global:LASTEXITCODE = 0; return
  }
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
    if ($env:BACKEND -ne 'old-claude') { '--tools <tools> --disallowedTools <tools> --permission-mode <mode>' }
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
function pi {
  if (($args -join ' ') -eq '--help') {
    if ($env:BACKEND -ne 'pi-old') { '--model <pattern> --tools, -t <tools> --exclude-tools, -xt <tools> --session-id <id>' }
    else { '--model <pattern> --session-id <id>' }
    $global:LASTEXITCODE = 0; return
  }
  Set-Content $env:CANARY TAMPERED
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
      echo "FAIL Windows $BACKEND: canary changed"; exit 1
    fi
    case "$BACKEND" in
      old|old-claude)
        if [ "$rc" -eq 0 ] || [ -s "$CALLS" ]; then cat "$tmp/win-out"; echo "FAIL Windows $BACKEND: not refused"; exit 1; fi ;;
      mutate)
        if [ ! -s "$CALLS" ] || [ "$rc" -eq 0 ] || ! grep -q '^WORKTREE_CHECK=failed' "$tmp/scratch/review-pr9998-r1.done"; then cat "$tmp/win-out"; echo 'FAIL Windows mutate: source change was accepted'; exit 1; fi ;;
      *)
        if [ ! -s "$CALLS" ] || [ "$rc" -ne 0 ]; then cat "$tmp/win-out"; echo "FAIL Windows $BACKEND: no successful launch"; exit 1; fi
        grep -q '^TOOL_FLAGS=.*--permission-mode.*mcp__' "$tmp/scratch/review-pr9998-r1.done" || exit 1 ;;
    esac
  done
  # GH-880 pi arm canaries against the pi-generated lane. The pi lane
  # generation overwrites the claude lane file after the claude loop above
  # has finished with it.
  TEST_WIN=1 EDDA_REVIEW_AGENT=pi EDDA_REVIEW_MODEL=openrouter/z-ai/glm-5.3-flash sh "$root/scripts/review-pr.sh" 9998 --dry-run > "$tmp/generated-win-pi"
  for BACKEND in pi-modern pi-old pi-old-edda; do
    export BACKEND
    printf ORIGINAL > "$CANARY"
    : > "$CALLS"
    printf 'dirty-before-review\n' > "$tmp/scratch/wt-review-pr9998/tracked.txt"
    printf 'untracked-before-review\n' > "$tmp/scratch/wt-review-pr9998/untracked.txt"
    printf 'ignored-before-review\n' > "$tmp/scratch/wt-review-pr9998/ignored.txt"
    echo brief > "$tmp/scratch/review-pr9998-r1-brief.md"
    rc=0
    pwsh -NoProfile -File "$(cygpath -w "$tmp/driver.ps1")" \
      -Lane "$(cygpath -w "$tmp/scratch/review-pr9998-r1-lane.ps1")" > "$tmp/win-out-pi" 2>&1 || rc=$?
    if [ "$(tr -d '\r\n' < "$CANARY")" != ORIGINAL ]; then
      echo "FAIL Windows $BACKEND: canary changed"; exit 1
    fi
    case "$BACKEND" in
      pi-modern)
        if [ ! -s "$CALLS" ] || [ "$rc" -ne 0 ]; then cat "$tmp/win-out-pi"; echo "FAIL Windows $BACKEND: no successful pi launch"; exit 1; fi
        grep -q '^TRANSPORT=pi-dispatch' "$tmp/scratch/review-pr9998-r1.done" || { cat "$tmp/win-out-pi"; echo 'FAIL Windows pi-modern: no TRANSPORT=pi-dispatch receipt'; exit 1; }
        grep -qF "TOOL_FLAGS=--tools 'read,grep,find,ls'" "$tmp/scratch/review-pr9998-r1.done" || { cat "$tmp/win-out-pi"; echo 'FAIL Windows pi-modern: TOOL_FLAGS receipt missing the allowlist'; exit 1; } ;;
      *)
        if [ "$rc" -eq 0 ] || [ -s "$CALLS" ]; then cat "$tmp/win-out-pi"; echo "FAIL Windows $BACKEND: not refused"; exit 1; fi ;;
    esac
  done
  echo 'Windows generated lane canaries passed (old/modern transport and source-snapshot cases; pi-modern launch and pi-old/pi-old-edda refusals)'
fi
