#!/bin/sh
# GH-702: execute the generated POSIX runner against an old/fake backend.
# An overprivileged launch actually alters the canary; help probing is free.
set -eu
root=$(CDPATH= cd -- "$(dirname "$0")/.." && pwd)
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' 0 HUP INT TERM
mkdir -p "$tmp/bin" "$tmp/scratch/wt-review-pr9998"
git -C "$tmp/scratch/wt-review-pr9998" init -q
git -C "$tmp/scratch/wt-review-pr9998" -c user.name=fixture -c user.email=fixture@example.invalid \
  commit --allow-empty -qm fixture
export GH_HEAD=$(git -C "$tmp/scratch/wt-review-pr9998" rev-parse HEAD)
export EDDA_FLEET_SCRATCH="$tmp/scratch" EDDA_REVIEW_SPEC="$root/REVIEW.md"
export EDDA_FLEET_ROOT="$root" CANARY="$tmp/canary" CALLS="$tmp/calls"
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
 [ "$BACKEND" = old ] || echo '--tools <TOOLS> --exclude-tools <EXCLUDE_TOOLS>'
 exit 0
fi
echo launch >> "$CALLS"
case "$*" in
 *"--tools Read,Grep,Glob,Bash(git *),Bash(gh *),Bash(edda *),Bash(sh *)"*"--exclude-tools Edit,Write,NotebookEdit,mcp__*"*) : ;;
 *) echo TAMPERED > "$CANARY" ;;
esac
EOF
cat > "$tmp/bin/claude" <<'EOF'
#!/bin/sh
if [ "$*" = '--help' ]; then
 [ "$BACKEND" = old-claude ] || echo '--tools <tools> --allowedTools <tools> --disallowedTools <tools>'
 exit 0
fi
echo launch >> "$CALLS"
case "$*" in
 *"--tools Read,Grep,Glob,Bash(git *),Bash(gh *),Bash(edda *),Bash(sh *)"*"--disallowedTools Edit,Write,NotebookEdit,mcp__*"*) : ;;
 *) echo TAMPERED > "$CANARY" ;;
esac
printf '{"result":"fixture","session_id":"fixture"}\n'
EOF
chmod +x "$tmp/bin/"*
export PATH="$tmp/bin:$PATH"
sh "$root/scripts/review-pr.sh" 9998 --dry-run > "$tmp/generated"
runner="$tmp/scratch/review-pr9998-r1-run.sh"
# Suppress cleanup only: the fixture cwd is not a registered worktree.
sed '/git -C .* worktree remove /d' "$runner" > "$tmp/run.sh"
failures=0
for BACKEND in old modern old-claude fallback; do
 export BACKEND
 echo ORIGINAL > "$CANARY"
 : > "$CALLS"
 if [ "$BACKEND" = old-claude ] || [ "$BACKEND" = fallback ]; then
   head -c 31000 /dev/zero > "$tmp/scratch/review-pr9998-r1-brief.md"
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
 *)
   if [ ! -s "$CALLS" ] || [ "$rc" -ne 0 ]; then
     echo "FAIL $BACKEND: capable backend never launched"; failures=$((failures + 1))
   fi
   if ! grep -q '^TOOL_FLAGS=.*mcp__' "$tmp/scratch/review-pr9998-r1.done"; then
     echo "FAIL $BACKEND: no actual flags receipt"; failures=$((failures + 1))
   fi ;;
 esac
done
[ "$failures" -eq 0 ] || exit 1
echo 'review capability canaries passed (old dispatch, modern dispatch, old fallback, modern fallback)'

# On Windows execute the generated PowerShell lane too. Functions stand in
# for the two binaries; any non-help call with missing flags writes canary.
if command -v pwsh >/dev/null 2>&1 && command -v cygpath >/dev/null 2>&1; then
  TEST_WIN=1 sh "$root/scripts/review-pr.sh" 9998 --dry-run > "$tmp/generated-win"
  cat > "$tmp/driver.ps1" <<'EOF'
param([string]$Lane)
function edda {
  if (($args -join ' ') -eq 'dispatch --help') {
    if ($env:BACKEND -ne 'old') { '--tools <TOOLS> --exclude-tools <TOOLS>' }
    $global:LASTEXITCODE = 0; return
  }
  Add-Content $env:CALLS launch
  if ($args -notcontains $expectedTools -or $args -notcontains 'Edit,Write,NotebookEdit,mcp__*') { Set-Content $env:CANARY TAMPERED }
  $global:LASTEXITCODE = 0
}
function claude {
  if (($args -join ' ') -eq '--help') {
    if ($env:BACKEND -ne 'old-claude') { '--tools <tools> --disallowedTools <tools>' }
    $global:LASTEXITCODE = 0; return
  }
  Add-Content $env:CALLS launch
  if ($args -notcontains $expectedTools -or $args -notcontains 'Edit,Write,NotebookEdit,mcp__*') { Set-Content $env:CANARY TAMPERED }
  '{"result":"fixture","session_id":"fixture","modelUsage":{"fixture":{}}}'
  $global:LASTEXITCODE = 0
}
$expectedTools = 'Read,Grep,Glob,Bash(git *),Bash(gh *),Bash(edda *),Bash(sh *)'
& $Lane
exit $LASTEXITCODE
EOF
  CANARY=$(cygpath -w "$CANARY"); export CANARY
  CALLS=$(cygpath -w "$CALLS"); export CALLS
  for BACKEND in old modern old-claude fallback; do
    export BACKEND
    printf ORIGINAL > "$CANARY"
    : > "$CALLS"
    if [ "$BACKEND" = old ] || [ "$BACKEND" = modern ]; then
      echo brief > "$tmp/scratch/review-pr9998-r1-brief.md"
    else
      head -c 31000 /dev/zero > "$tmp/scratch/review-pr9998-r1-brief.md"
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
      *)
        if [ ! -s "$CALLS" ] || [ "$rc" -ne 0 ]; then cat "$tmp/win-out"; echo "FAIL Windows $BACKEND: no successful launch"; exit 1; fi
        grep -q '^TOOL_FLAGS=.*mcp__' "$tmp/scratch/review-pr9998-r1.done" || exit 1 ;;
    esac
  done
  echo 'Windows generated lane canaries passed (four transport/capability cases)'
fi
