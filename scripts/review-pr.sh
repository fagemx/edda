#!/bin/sh
# review-pr.sh — launch one read-only review of a PR (gpt-5.6-sol via pi), per
# fleet.lane-launch (Task Scheduler, hidden), fleet.review-brief-framing
# (validation-checklist brief) and fleet.agent-model-split (review model is fixed).
#
# The brief is BUILT BY READING REVIEW.md, the repo's executable review spec.
# This script contributes only the PR's facts (head SHA, surface, class, the
# linked issue's doneWhen); every review rule, severity, check command and the
# output format come from REVIEW.md, which is inlined verbatim into the brief.
# A missing REVIEW.md is a hard error — a brief without the spec would silently
# review against nothing.
#
# usage: review-pr.sh <PR> [round] [prev-sha] [--sha <full-sha>] [--dry-run]
#        review-pr.sh verdict-label < verdict-text        (offline helper)
#
# Environment:
#   EDDA_REPO              owner/repo              (default fagemx/edda)
#   EDDA_FLEET_ROOT        main checkout path      (default: derived from git)
#   EDDA_FLEET_SCRATCH     state/log dir           (default $HOME/.edda/fleet)
#   EDDA_REVIEW_MODEL      review model            (default openai-codex/gpt-5.6-sol)
#   EDDA_REVIEW_SPEC       path to REVIEW.md       (default: repo root beside this script)
#
# Outputs (under $EDDA_FLEET_SCRATCH):
#   review-pr<N>-r<R>-brief.md     the review brief fed to the reviewer
#   review-pr<N>-r<R>.log          pi console transcript (verdict is between
#                                  <<<VERDICT and VERDICT>>> markers)
#   review-pr<N>-r<R>.done         written when pi exits ("PI_EXIT=<code>")
#   wt-review-pr<N>/               detached worktree at the PR head
#
# --dry-run generates the brief and prints what would be launched, but does not
# create the worktree, register the scheduled task, or start any process.
set -u

usage() {
  echo "usage: review-pr.sh <PR> [round] [prev-sha] [--dry-run]" >&2
}

REPO=${EDDA_REPO:-fagemx/edda}
MODEL=${EDDA_REVIEW_MODEL:-openai-codex/gpt-5.6-sol}
MODEL_SHORT=${MODEL##*/}
SCRATCH=${EDDA_FLEET_SCRATCH:-$HOME/.edda/fleet}

# The review spec lives at the repo root, beside this script's parent. Resolve
# it from the script's own location so a detached worktree, a scheduled task or
# any cwd still finds the same file.
SELF_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
SPEC=${EDDA_REVIEW_SPEC:-$SELF_DIR/../REVIEW.md}

PR=""
ROUND=1
PREV=""
DRY=0
SHA_GIVEN=""

# Offline helper: map verdict text (stdin) to a review:* label; the LAST
# verdict keyword in the text wins. No network, no state.
if [ "${1:-}" = "verdict-label" ]; then
  v=$(cat)
  lcs=$(printf '%s\n' "$v" | grep -bo "Changes Requested" | tail -1 | cut -d: -f1)
  llg=$(printf '%s\n' "$v" | grep -bo "LGTM" | tail -1 | cut -d: -f1)
  if [ -n "$lcs" ] && [ -n "$llg" ] && [ "$llg" -gt "$lcs" ]; then echo "review:lgtm"
  elif [ -n "$lcs" ]; then echo "review:changes-requested"
  elif [ -n "$llg" ]; then echo "review:lgtm"
  fi
  exit 0
fi

while [ $# -gt 0 ]; do
  case "$1" in
    --dry-run) DRY=1 ;;
    --sha) SHA_GIVEN=${2:-}; shift ;;
    -h|--help) usage; exit 0 ;;
    *)
      if [ -z "$PR" ]; then PR=$1
      elif [ "$ROUND" = "1" ] && [ -z "${ROUND_SET:-}" ]; then ROUND=$1; ROUND_SET=1
      elif [ -z "$PREV" ]; then PREV=$1
      else echo "review-pr.sh: unexpected argument '$1'" >&2; usage; exit 1; fi
      ;;
  esac
  shift
done
if [ -z "$PR" ]; then usage; exit 1; fi
case "$PR$ROUND" in
  *[!0-9]*) echo "review-pr.sh: PR and round must be numeric" >&2; exit 1 ;;
esac
if [ -n "$SHA_GIVEN" ]; then
  printf '%s\n' "$SHA_GIVEN" | grep -qE '^[0-9a-f]{40}$' || {
    echo "review-pr.sh: --sha must be a full 40-hex SHA" >&2; exit 1;
  }
fi

# ---- platform ---------------------------------------------------------------
IS_WIN=0
case "$(uname -s)" in
  MINGW*|MSYS*|CYGWIN*) IS_WIN=1 ;;
esac

# ---- repo root (main checkout; needed for the detached worktree) ------------
ROOT=${EDDA_FLEET_ROOT:-}
if [ -z "$ROOT" ]; then
  common=$(git rev-parse --path-format=absolute --git-common-dir 2>/dev/null) || common=""
  [ -n "$common" ] && ROOT=$(dirname "$common")
fi

# ---- PR facts ---------------------------------------------------------------
# The reviewed SHA is pinned by the caller (--sha from the watcher's scan); a
# second head read is used ONLY to refuse a stale pin (head moved), never to
# pick what gets reviewed.
if [ -n "$SHA_GIVEN" ]; then
  SHA=$SHA_GIVEN
  CUR=$(gh pr view "$PR" --repo "$REPO" --json headRefOid --jq .headRefOid 2>/dev/null) || CUR=""
  if [ "$CUR" != "$SHA" ]; then
    echo "review-pr.sh: head moved — refusing to review pinned $SHA; PR head is ${CUR:-unknown}" >&2
    exit 3
  fi
else
  SHA=$(gh pr view "$PR" --repo "$REPO" --json headRefOid --jq .headRefOid) ||
    { echo "review-pr.sh: cannot read PR #$PR (repo $REPO)" >&2; exit 1; }
fi
BR=$(gh pr view "$PR" --repo "$REPO" --json headRefName --jq .headRefName)
TITLE=$(gh pr view "$PR" --repo "$REPO" --json title --jq .title)
BODY=$(gh pr view "$PR" --repo "$REPO" --json body --jq .body)

# Issue numbers from the PR body's "Issue: #N" lines.
ISSUES=$(printf '%s\n' "$BODY" \
  | awk 'tolower($0) ~ /^issue[[:space:]]*:/ || tolower($0) ~ /^issues[[:space:]]*:/' \
  | grep -Eo '#[0-9]+' | tr -d '#' | sort -u)
# Allowed surface = the PR's changed files.
FILES=$(gh pr diff "$PR" --repo "$REPO" --name-only)
SURFACE=$(printf '%s\n' "$FILES" | paste -sd, - | sed 's/,$//')

# ---- class routing (REVIEW.md §3, mechanical path rule) ---------------------
# Same case arms as REVIEW.md §3; a mixed diff carries every class it matches
# and an unrecognised path defaults to code-plain (never to prose).
classify() {
  risk=""; plain=""; skills=""; docs=""
  while IFS= read -r f; do
    [ -n "$f" ] || continue
    case "$f" in
      *.sh|*.ps1|*.bash|scripts/*|.github/*|lefthook.yml) risk=" code-risk" ;;
      Cargo.toml|Cargo.lock|*/Cargo.toml|install.sh|.gitignore|.gitattributes) risk=" code-risk" ;;
      crates/edda-ledger/*|crates/edda-store/*|crates/edda-core/*) risk=" code-risk" ;;
      crates/edda-conductor/*|crates/edda-cli/src/cmd_dispatch.rs) risk=" code-risk" ;;
      *.rs|*.toml|*.lock) plain=" code-plain" ;;
      */SKILL.md|.claude/skills/*|skills/*) skills=" skills" ;;
      .claude/CLAUDE.md|AGENTS.md|CLAUDE.md|REVIEW.md) skills=" skills" ;;
      *.md|docs/*|*.txt|LICENSE*) docs=" docs" ;;
      *) plain=" code-plain" ;;
    esac
  done
  printf '%s\n' "${risk}${plain}${skills}${docs}" | sed 's/^ //'
}
CLASSES=$(printf '%s\n' "$FILES" | classify)
# REVIEW.md §3.2: calibration data is keyed on the two canonical classes.
case "$CLASSES" in
  *code-risk*|*code-plain*) CANON_CLASS="code-risk" ;;
  *)                        CANON_CLASS="docs-skills" ;;
esac

# ---- brief: PR facts + REVIEW.md verbatim (fleet.review-brief-framing) ------
# REVIEW.md is the whole rule set. Refuse rather than emit a spec-less brief.
if [ ! -f "$SPEC" ]; then
  echo "review-pr.sh: review spec not found at $SPEC (set EDDA_REVIEW_SPEC)" >&2
  exit 1
fi
SPEC_VERSION=$(sed -n 's/^- Spec version: `\(.*\)`$/\1/p' "$SPEC" | head -1)
if [ -z "$SPEC_VERSION" ]; then
  echo "review-pr.sh: $SPEC has no '- Spec version: \`...\`' line" >&2
  exit 1
fi

mkdir -p "$SCRATCH"
BRIEF="$SCRATCH/review-pr$PR-r$ROUND-brief.md"
SID="review-pr$PR"
{
  echo "# Review brief — PR #$PR, Round $ROUND (read-only, $MODEL_SHORT, session $SID)"
  echo
  echo "You stand in a detached worktree at PR head **$SHA** (branch \`$BR\`). The workspace ledger is reachable (\`edda ask\` works). Title: $TITLE. Issue(s): ${ISSUES:-none}. Files changed by the PR: ${SURFACE:-none}"
  if [ -n "$PREV" ]; then
    echo "This is a DELTA round: your prior verdict on $PREV is posted on the PR; review \`git diff $PREV..$SHA\` and RAN-confirm each prior finding is resolved; do not re-review the whole PR."
  fi
  echo
  echo "## The spec you run"
  echo "This review is **REVIEW.md ($SPEC_VERSION)**, reproduced verbatim in the SPEC section at the end of this brief. Run it top to bottom. It is the only source of review rules, severities, check commands and output format; everything above and below it in this brief is only this PR's facts."
  echo
  echo "- Classification (REVIEW.md §3 router, already run on the changed files): **${CLASSES:-code-plain}** — run every rule section §4 routes those classes to. You may up-class with a stated reason; never down-class."
  echo "- Canonical \`class:\` field for the verdict (REVIEW.md §3.2): **$CANON_CLASS**"
  echo "- \`spec:\` field for the verdict: **$SPEC_VERSION**"
  echo "- Allowed surface (rule U1): \`${SURFACE:-<empty>}\` — a changed file outside it is a P0."
  echo "- Read-only (REVIEW.md §0): no edits, no pushes, no GitHub posts, no cargo, no merge. Budget ~6 minutes."
  for i in $ISSUES; do
    echo
    echo "### Issue #$i doneWhen (the acceptance ceiling, REVIEW.md §1)"
    gh issue view "$i" --repo "$REPO" --json body --jq .body 2>/dev/null \
      | awk '/^## doneWhen/{f=1;next} /^## /{f=0} f' || echo "(issue #$i could not be read)"
  done
  echo
  echo "## Output"
  echo "Print the REVIEW.md §7 verdict — every field, the Rules table with one row per routed rule, the Wiring table — between the markers below, with this header line filled in:"
  echo "<<<VERDICT"
  echo "## Code Review: Round $ROUND — PR #$PR @ $SHA"
  echo "…the rest exactly as REVIEW.md §7 specifies (model_requested: $MODEL, spec: $SPEC_VERSION, class: $CANON_CLASS)…"
  echo "VERDICT>>>"
  echo
  echo "---"
  echo
  echo "# SPEC — REVIEW.md ($SPEC_VERSION), verbatim"
  echo
  cat "$SPEC"
} > "$BRIEF"

echo "brief=$BRIEF"
echo "sha=$SHA"
echo "issues=${ISSUES:-none}"
echo "surface=${SURFACE:-none}"
echo "classes=${CLASSES:-code-plain}"
echo "canonical_class=$CANON_CLASS"
echo "spec=$SPEC ($SPEC_VERSION)"

if [ "$DRY" = "1" ]; then
  echo "dry-run: brief generated; nothing launched, no worktree, no scheduled task."
  exit 0
fi

if [ -z "$ROOT" ]; then
  echo "review-pr.sh: cannot locate the main checkout (set EDDA_FLEET_ROOT)" >&2
  exit 1
fi

# ---- detached worktree at the PR head ---------------------------------------
WT="$SCRATCH/wt-review-pr$PR"
git -C "$ROOT" fetch -q origin "$BR"
if [ -d "$WT" ]; then
  git -C "$WT" checkout -q --detach "$SHA"
else
  git -C "$ROOT" worktree add --detach "$WT" "$SHA" >/dev/null
fi

PROMPT="Use your read tool to read $BRIEF and follow it exactly. Work read-only in the current directory (a detached worktree at PR #$PR head). Finish by printing the verdict between the <<<VERDICT and VERDICT>>> markers."

if [ "$IS_WIN" = "1" ]; then
  # Windows: Task Scheduler, not nohup (fleet.lane-launch). HOME and UTF-8 are
  # empty/CP950 in the scheduled-task environment and must be set explicitly.
  command -v pwsh >/dev/null 2>&1 || { echo "review-pr.sh: pwsh not found" >&2; exit 1; }
  command -v cygpath >/dev/null 2>&1 || { echo "review-pr.sh: cygpath not found" >&2; exit 1; }
  WTW=$(cygpath -w "$WT")
  BRIEFW=$(cygpath -w "$BRIEF")
  LOGW=$(cygpath -w "$SCRATCH\\review-pr$PR-r$ROUND.log")
  DONEW=$(cygpath -w "$SCRATCH\\review-pr$PR-r$ROUND.done")
  TASK="edda-review-pr$PR-r$ROUND"

  cat > "$SCRATCH/review-pr$PR-r$ROUND-lane.ps1" <<PS
[Console]::InputEncoding = [System.Text.UTF8Encoding]::new(\$false)
[Console]::OutputEncoding = [System.Text.UTF8Encoding]::new(\$false)
\$OutputEncoding = [System.Text.UTF8Encoding]::new(\$false)
\$env:HOME = \$env:USERPROFILE
Set-Location '$WTW'
\$prompt = "$PROMPT"
& pi -p --model $MODEL --thinking high --exclude-tools edit,write --session-id $SID \$prompt 2>&1 | Out-File -FilePath "$LOGW" -Encoding utf8
"PI_EXIT=\$LASTEXITCODE" | Out-File "$DONEW" -Encoding utf8
PS

  cat > "$SCRATCH/review-pr$PR-r$ROUND-launch.ps1" <<PS
foreach (\$f in @("$LOGW", "$DONEW")) { if (Test-Path \$f) { [System.IO.File]::Delete(\$f) } }
Unregister-ScheduledTask -TaskName '$TASK' -Confirm:\$false -ErrorAction SilentlyContinue
\$action = New-ScheduledTaskAction -Execute "pwsh.exe" -Argument "-NoProfile -NonInteractive -WindowStyle Hidden -ExecutionPolicy Bypass -File \`"$SCRATCH\\review-pr$PR-r$ROUND-lane.ps1\`"" -WorkingDirectory '$WTW'
\$settings = New-ScheduledTaskSettingsSet -ExecutionTimeLimit (New-TimeSpan -Minutes 30) -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries
Register-ScheduledTask -TaskName '$TASK' -Action \$action -Settings \$settings -RunLevel Limited | Out-Null
Start-ScheduledTask -TaskName '$TASK'
\$st = ""
for (\$i = 0; \$i -lt 20; \$i++) {
  Start-Sleep -Seconds 1
  \$st = (Get-ScheduledTask -TaskName '$TASK').State
  if (\$st -eq 'Running') { break }
}
"task=$TASK state=\$st"
PS

  out=$(pwsh -NoProfile -ExecutionPolicy Bypass -File "$SCRATCH/review-pr$PR-r$ROUND-launch.ps1" 2>&1); rc=$?
  echo "$out"
  [ $rc -eq 0 ] || exit 1
  case "$out" in
    *state=Running*) : ;;
    *) echo "review-pr.sh: scheduled task did not reach Running" >&2; exit 1 ;;
  esac
else
  # Linux: no job-object trap, plain nohup is enough.
  RUNNER="$SCRATCH/review-pr$PR-r$ROUND-run.sh"
  cat > "$RUNNER" <<RUN
#!/bin/sh
export HOME="\${HOME:-$(getent passwd "\$(id -u)" 2>/dev/null | cut -d: -f6)}"
cd '$WT' || exit 1
pi -p --model '$MODEL' --thinking high --exclude-tools edit,write --session-id '$SID' "$PROMPT" > '$SCRATCH/review-pr$PR-r$ROUND.log' 2>&1
echo "PI_EXIT=\$?" > '$SCRATCH/review-pr$PR-r$ROUND.done'
RUN
  sh -n "$RUNNER" || exit 1
  chmod +x "$RUNNER"
  rm -f "$SCRATCH/review-pr$PR-r$ROUND.log" "$SCRATCH/review-pr$PR-r$ROUND.done"
  nohup "$RUNNER" >/dev/null 2>&1 &
  pid=$!
  sleep 1
  if kill -0 "$pid" 2>/dev/null; then
    echo "task=nohup pid=$pid state=Running"
  else
    echo "review-pr.sh: nohup process $pid died immediately" >&2
    exit 1
  fi
fi

echo "log=$SCRATCH/review-pr$PR-r$ROUND.log"
echo "done=$SCRATCH/review-pr$PR-r$ROUND.done"
echo "session=$SID"
