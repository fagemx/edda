#!/bin/sh
# review-pr.sh — launch one read-only review of a PR (claude-opus-5 via
# `edda dispatch --agent claude`), per fleet.lane-launch (Task Scheduler,
# hidden), fleet.review-brief-framing (validation-checklist brief) and
# fleet.review-engine-model / fleet.review-backend (the review model is fixed
# and explicitly pinned with --model; reviews run on the Claude subscription
# because pi's openrouter routing cannot reach any Anthropic model — GH-708).
#
# The brief is BUILT BY READING REVIEW.md, the repo's executable review spec.
# This script contributes only the PR's facts (head SHA, surface, the linked
# issue's doneWhen); every review rule, severity, check command, the class
# router and the output format come from REVIEW.md, which is inlined verbatim
# into the brief. A missing REVIEW.md is a hard error — a brief without the
# spec would silently review against nothing.
#
# REVIEW.md is read AT THE PR'S BASE SHA, never at the head (decision
# review.brief-source; docs/superpowers/specs/2026-09-02-edda-review-design.md
# §5), so a PR cannot rewrite the rules it is judged by. The class router is
# not reimplemented here either: it is extracted from the spec's
# "# review-spec:classifier" block and run as-is, so REVIEW.md §3 stays the
# only copy.
#
# usage: review-pr.sh <PR> [round] [prev-sha] [--sha <full-sha>] [--dry-run]
#        review-pr.sh verdict-label < verdict-text        (offline helper)
#
# Environment:
#   EDDA_REPO              owner/repo              (default fagemx/edda)
#   EDDA_FLEET_ROOT        main checkout path      (default: derived from git)
#   EDDA_FLEET_SCRATCH     state/log dir           (default $HOME/.edda/fleet)
#   EDDA_REVIEW_MODEL      review model            (default claude-opus-5; passed
#                                                  to `edda dispatch --model`)
#   EDDA_REVIEW_SPEC       explicit REVIEW.md path (override; default: read
#                                                  REVIEW.md at the PR base SHA)
#
# Outputs (under $EDDA_FLEET_SCRATCH):
#   review-pr<N>-r<R>-brief.md     the review brief fed to the reviewer
#   review-pr<N>-r<R>.log          reviewer console transcript (verdict is between
#                                  <<<VERDICT and VERDICT>>> markers; the
#                                  dispatch receipt's `Model requested:` /
#                                  `Model observed:` / `Cost:` lines follow it)
#   review-pr<N>-r<R>.done         written when the dispatch exits
#                                  ("DISPATCH_EXIT=<code>")
#   wt-review-pr<N>/               detached worktree at the PR head
#
# --dry-run generates the brief and prints what would be launched, but does not
# create the worktree, register the scheduled task, or start any process.
set -u

usage() {
  echo "usage: review-pr.sh <PR> [round] [prev-sha] [--dry-run]" >&2
}

REPO=${EDDA_REPO:-fagemx/edda}
MODEL=${EDDA_REVIEW_MODEL:-claude-opus-5}
MODEL_SHORT=${MODEL##*/}
SCRATCH=${EDDA_FLEET_SCRATCH:-$HOME/.edda/fleet}

# The spec is read out of git, not off the working tree. Resolve the checkout
# from this script's own location so a detached worktree, a scheduled task or
# any cwd still reaches the same object database.
SELF_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
SELF_REPO=$(CDPATH= cd -- "$SELF_DIR/.." && pwd)
SPEC_OVERRIDE=${EDDA_REVIEW_SPEC:-}

PR=""
ROUND=1
PREV=""
DRY=0
SHA_GIVEN=""

# Offline helper: map verdict text (stdin) to a review:* label. No network, no
# state.
#
# The label is read from the FIRST verdict line under the `### Verdict` heading
# REVIEW.md §7 mandates — never from the last verdict keyword in the text.
# "Last keyword wins" inverted the label on PR #655, whose Verdict line said
# `Changes Requested, P0=1, P1=1` and whose prose ended "that alone should
# carry this to LGTM": the PR was labelled `review:lgtm` while carrying an open
# P0 (issue #697). "not yet LGTM", "re-review for LGTM once fixed" and "would
# carry this to LGTM" are all ordinary reviewer prose and every one of them
# inverted it. The verdict is a specific line in a specific section.
#
# Input is one round's verdict text; the first `### Verdict` heading is the one
# read. No Verdict line at all is NOT an LGTM: emit nothing and exit 0, and let
# the caller decide (the watcher labels `review:unreviewed` / `review:post-
# failed` rather than guessing). `Changes Requested` is tested first so a line
# naming both resolves to the blocking side.
if [ "${1:-}" = "verdict-label" ]; then
  vline=$(sed -n '/^#\{1,\}[[:space:]]*Verdict/,$p' | sed '1d' \
          | grep -m1 -E 'LGTM|Changes Requested') || vline=""
  case "$vline" in
    *"Changes Requested"*) echo "review:changes-requested" ;;
    *LGTM*)                echo "review:lgtm" ;;
  esac
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
BASE_REF=$(gh pr view "$PR" --repo "$REPO" --json baseRefName --jq .baseRefName)
BASE_SHA=$(gh pr view "$PR" --repo "$REPO" --json baseRefOid --jq .baseRefOid)
TITLE=$(gh pr view "$PR" --repo "$REPO" --json title --jq .title)
BODY=$(gh pr view "$PR" --repo "$REPO" --json body --jq .body)

# Issue numbers — the acceptance ceiling (REVIEW.md §1). Collected from three
# sources, because collecting from only the repo's `Issue: #N` convention made
# the contract fail open: PR #670's body opened `Closes #650.` — the form
# GitHub itself recognises — no line matched, and the brief was generated with
# no doneWhen at all while still telling the reviewer to judge against one
# (issue #683).
#
#   1. the repo's own `Issue:` / `Issues:` lines;
#   2. GitHub's closing keywords anywhere in the body;
#   3. GitHub's own linkage (closingIssuesReferences), which also catches an
#      issue linked from the sidebar with nothing in the body.
#
# Bare `#N` prose references are deliberately NOT mined. A body's "Related:
# #641 and #632" names a sibling PR and a tracking issue, not this PR's
# acceptance ceiling; pulling their doneWhen into the brief would hand the
# reviewer the wrong ceiling, which is worse than handing it none.
CLOSING_KW='closes|closed|close|fixes|fixed|fix|resolves|resolved|resolve'
ISSUES=$(
  {
    printf '%s\n' "$BODY" \
      | awk 'tolower($0) ~ /^issue[[:space:]]*:/ || tolower($0) ~ /^issues[[:space:]]*:/' \
      | grep -Eo '#[0-9]+' | tr -d '#'
    printf '%s\n' "$BODY" \
      | grep -Eio "($CLOSING_KW)[[:space:]]+[a-z0-9_.-]*(/[a-z0-9_.-]+)?#[0-9]+" \
      | grep -Eo '#[0-9]+' | tr -d '#'
    gh pr view "$PR" --repo "$REPO" --json closingIssuesReferences \
      --jq '.closingIssuesReferences[].number' 2>/dev/null
  } | grep -E '^[0-9]+$' | sort -un
)
# Allowed surface = the PR's changed files.
FILES=$(gh pr diff "$PR" --repo "$REPO" --name-only)
SURFACE=$(printf '%s\n' "$FILES" | paste -sd, - | sed 's/,$//')

# ---- the spec, read at the BASE SHA -----------------------------------------
# review.brief-source: REVIEW.md is always the base_sha version, never the head,
# so a PR cannot rewrite the rules it will be judged by. EDDA_REVIEW_SPEC is an
# explicit file override (offline tests, a spec under development).
mkdir -p "$SCRATCH"
SPEC="$SCRATCH/review-pr$PR-r$ROUND-spec.md"
if [ -n "$SPEC_OVERRIDE" ]; then
  if [ ! -f "$SPEC_OVERRIDE" ]; then
    echo "review-pr.sh: review spec not found at $SPEC_OVERRIDE (EDDA_REVIEW_SPEC)" >&2
    exit 1
  fi
  SPEC=$SPEC_OVERRIDE
  SPEC_SOURCE="$SPEC_OVERRIDE (EDDA_REVIEW_SPEC override)"
else
  # The base commit must be in this object database before it can be read.
  git -C "$SELF_REPO" cat-file -e "$BASE_SHA^{commit}" 2>/dev/null ||
    git -C "$SELF_REPO" fetch -q origin "$BASE_REF" 2>/dev/null || true
  if git -C "$SELF_REPO" show "$BASE_SHA:REVIEW.md" > "$SPEC" 2>/dev/null; then
    SPEC_SOURCE="REVIEW.md@$BASE_SHA (base of $BASE_REF)"
  elif [ -f "$SELF_REPO/REVIEW.md" ]; then
    # The base predates REVIEW.md (or the object is unreachable). Fall back to
    # the checkout's copy, loudly — never emit a spec-less brief, and never let
    # the substitution pass unnoticed.
    cp "$SELF_REPO/REVIEW.md" "$SPEC"
    SPEC_SOURCE="$SELF_REPO/REVIEW.md (FALLBACK: no REVIEW.md at base $BASE_SHA)"
    echo "review-pr.sh: no REVIEW.md at base $BASE_SHA; falling back to the checkout copy" >&2
  else
    echo "review-pr.sh: no REVIEW.md at base $BASE_SHA and none in $SELF_REPO (set EDDA_REVIEW_SPEC)" >&2
    exit 1
  fi
fi
SPEC_VERSION=$(sed -n 's/^- Spec version: `\(.*\)`$/\1/p' "$SPEC" | head -1)
if [ -z "$SPEC_VERSION" ]; then
  echo "review-pr.sh: $SPEC_SOURCE has no '- Spec version: \`...\`' line" >&2
  exit 1
fi

# ---- class routing: REVIEW.md §3's own router, extracted and run ------------
# Not reimplemented here. The block between the two marker lines is the single
# copy of the router; it reads the file list on stdin and prints classes= and
# canonical_class= (REVIEW.md §3 and §3.2).
CLASSIFIER=$(awk '/^# review-spec:classifier$/{f=1;next} /^# review-spec:classifier-end$/{exit} f' "$SPEC")
if [ -z "$CLASSIFIER" ]; then
  echo "review-pr.sh: $SPEC_SOURCE has no '# review-spec:classifier' block" >&2
  exit 1
fi
ROUTED=$(printf '%s\n' "$FILES" | sh -c "$CLASSIFIER") || {
  echo "review-pr.sh: the REVIEW.md §3 classifier failed to run" >&2
  exit 1
}
CLASSES=$(printf '%s\n' "$ROUTED" | sed -n 's/^classes=//p')
CANON_CLASS=$(printf '%s\n' "$ROUTED" | sed -n 's/^canonical_class=//p')
if [ -z "$CLASSES" ] || [ -z "$CANON_CLASS" ]; then
  echo "review-pr.sh: the REVIEW.md §3 classifier printed no classes" >&2
  exit 1
fi

# ---- session id -------------------------------------------------------------
# The claude backend (`claude -p`) accepts only valid UUIDs: the old
# `review-pr$PR` shape made every launch die with "Invalid session ID. Must be
# a valid UUID." (issue #694's second half, walked into by GH-708's transport
# switch). A fresh UUID v4 per launch; rounds are SHA-pinned independent
# reviews, not resumed conversations.
gen_uuid() {
  r=$(od -An -N16 -tx1 /dev/urandom 2>/dev/null | tr -d ' \n')
  if [ "${#r}" -ne 32 ]; then
    echo "review-pr.sh: cannot read 16 random bytes for a session UUID" >&2
    exit 1
  fi
  # version nibble -> 4, variant nibble -> a (10xx)
  printf '%s-%s-4%s-a%s-%s\n' \
    "$(printf '%s' "$r" | cut -c 1-8)" \
    "$(printf '%s' "$r" | cut -c 9-12)" \
    "$(printf '%s' "$r" | cut -c 14-16)" \
    "$(printf '%s' "$r" | cut -c 18-20)" \
    "$(printf '%s' "$r" | cut -c 21-32)"
}
SID=$(gen_uuid)

# ---- brief: PR facts + REVIEW.md verbatim (fleet.review-brief-framing) ------
BRIEF="$SCRATCH/review-pr$PR-r$ROUND-brief.md"
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
  echo "- Spec source: \`$SPEC_SOURCE\` — read at the PR's **base** SHA, not at the head (decision \`review.brief-source\`), so this PR's own version of the rules is not the one judging it."
  case "$FILES" in
    *REVIEW.md*)
      echo "- **This diff changes \`REVIEW.md\`.** Per REVIEW.md §6.6 that adds the escalation \`REVIEW.md changed in this diff\` to your verdict, and you review under the base version above."
      ;;
  esac
  echo "- Classification (REVIEW.md §3 router, already run on the changed files): **${CLASSES:-code-plain}** — run every rule section §4 routes those classes to. You may up-class with a stated reason; never down-class."
  echo "- Canonical \`class:\` field for the verdict (REVIEW.md §3.2): **$CANON_CLASS**"
  echo "- \`spec:\` field for the verdict: **$SPEC_VERSION**"
  echo "- Allowed surface (rule U1): \`${SURFACE:-<empty>}\` — a changed file outside it is a P0."
  echo "- Read-only (REVIEW.md §0): no edits, no pushes, no GitHub posts, no cargo, no merge. Budget ~6 minutes."
  if [ -z "$ISSUES" ]; then
    # The contract must never reference a section that is not there. An empty
    # ceiling is stated, not omitted: a silently missing doneWhen makes a review
    # look complete when it judged against nothing at all (issue #683).
    echo
    echo "### No acceptance criteria found — this PR links no issue (REVIEW.md §1)"
    echo "This PR's body carries no \`Issue: #N\` line, no closing keyword, and GitHub reports no linked issue, so **no doneWhen was supplied to you**."
    echo "The acceptance ceiling REVIEW.md §1 tells you to judge against is MISSING, not empty. Do not read its absence as \"there is nothing to check\", and do not record a doneWhen row as PASS."
    echo "Obtain the issue and its doneWhen if you can. If you cannot, say so in the verdict and add \`no doneWhen available\` to \`escalations:\` — a review run without the ceiling is not a complete review."
  else
    for i in $ISSUES; do
      echo
      echo "### Issue #$i doneWhen (the acceptance ceiling, REVIEW.md §1)"
      gh issue view "$i" --repo "$REPO" --json body --jq .body 2>/dev/null \
        | awk '/^## doneWhen/{f=1;next} /^## /{f=0} f' || echo "(issue #$i could not be read)"
    done
  fi
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
  echo "# SPEC — REVIEW.md ($SPEC_VERSION), verbatim, from $SPEC_SOURCE"
  echo
  cat "$SPEC"
} > "$BRIEF"

echo "brief=$BRIEF"
echo "sha=$SHA"
echo "issues=${ISSUES:-none}"
echo "surface=${SURFACE:-none}"
echo "classes=${CLASSES:-code-plain}"
echo "canonical_class=$CANON_CLASS"
echo "spec=$SPEC_SOURCE ($SPEC_VERSION)"
echo "base=$BASE_SHA"

# ---- what would be launched: lane script + exact -File argument -------------
# Both launchers are GENERATED BEFORE the dry-run exit so the whole transport
# is inspectable offline: a POSIX -File argument is the #683 failure, and since
# GH-708 the lane body is also the receipt of which transport and model will
# run (`edda dispatch --agent claude --model ...`, session id = a real UUID).
# Writing the file starts nothing — a dry-run still creates no worktree,
# registers no scheduled task and starts no process.
WT="$SCRATCH/wt-review-pr$PR"
LOG="$SCRATCH/review-pr$PR-r$ROUND.log"
DONE="$SCRATCH/review-pr$PR-r$ROUND.done"
LANE="$SCRATCH/review-pr$PR-r$ROUND-lane.ps1"
LANE_FILE_ARG=$LANE
RUNNER="$SCRATCH/review-pr$PR-r$ROUND-run.sh"
if [ "$IS_WIN" = "1" ]; then
  command -v cygpath >/dev/null 2>&1 || { echo "review-pr.sh: cygpath not found" >&2; exit 1; }
  WTW=$(cygpath -w "$WT")
  BRIEFW=$(cygpath -w "$BRIEF")
  LOGW=$(cygpath -w "$LOG")
  DONEW=$(cygpath -w "$DONE")
  LANE_FILE_ARG=$(cygpath -w "$LANE")
  echo "lane_file_arg=$LANE_FILE_ARG"

  # LANE_FILE_ARG (above) is the cygpath -w form of $LANE. Building the task's
  # -File argument from the raw $SCRATCH instead — the one path here that used
  # not to be converted — yields "/c/Users/<user>/.edda/fleet\review-...ps1"
  # under Git Bash, which pwsh.exe cannot resolve (issue #683).
  cat > "$LANE" <<PS
[Console]::InputEncoding = [System.Text.UTF8Encoding]::new(\$false)
[Console]::OutputEncoding = [System.Text.UTF8Encoding]::new(\$false)
\$OutputEncoding = [System.Text.UTF8Encoding]::new(\$false)
\$env:HOME = \$env:USERPROFILE
Set-Location '$WTW'
& edda dispatch --agent claude --model '$MODEL' --exclude-tools 'Edit,Write,NotebookEdit' --session-id '$SID' --prompt-file "$BRIEFW" 2>&1 | Out-File -FilePath "$LOGW" -Encoding utf8
"DISPATCH_EXIT=\$LASTEXITCODE" | Out-File "$DONEW" -Encoding utf8
PS
else
  # Linux: no job-object trap, plain nohup is enough.
  cat > "$RUNNER" <<RUN
#!/bin/sh
export HOME="\${HOME:-$(getent passwd "\$(id -u)" 2>/dev/null | cut -d: -f6)}"
cd '$WT' || exit 1
edda dispatch --agent claude --model '$MODEL' --exclude-tools 'Edit,Write,NotebookEdit' --session-id '$SID' --prompt-file '$BRIEF' > '$LOG' 2>&1
echo "DISPATCH_EXIT=\$?" > '$DONE'
RUN
  sh -n "$RUNNER" || exit 1
  chmod +x "$RUNNER"
fi

if [ -z "$ISSUES" ]; then
  echo "review-pr.sh: WARNING: PR #$PR links no issue — the brief carries NO doneWhen, so this review has no acceptance ceiling (issue #683). The brief states this; obtain the issue before trusting the verdict." >&2
fi

if [ "$DRY" = "1" ]; then
  echo "dry-run: brief and launcher generated; nothing launched, no worktree, no scheduled task."
  exit 0
fi

if [ -z "$ROOT" ]; then
  echo "review-pr.sh: cannot locate the main checkout (set EDDA_FLEET_ROOT)" >&2
  exit 1
fi

command -v edda >/dev/null 2>&1 || {
  echo "review-pr.sh: edda not found on PATH — the review transport is 'edda dispatch --agent claude' (GH-708)" >&2
  exit 1
}

# ---- detached worktree at the PR head ---------------------------------------
git -C "$ROOT" fetch -q origin "$BR"
if [ -d "$WT" ]; then
  git -C "$WT" checkout -q --detach "$SHA"
else
  git -C "$ROOT" worktree add --detach "$WT" "$SHA" >/dev/null
fi

# A .done/.log left by an earlier attempt on the same PR/round is stale the
# moment a new launch starts. Deleted BEFORE the launch (the scheduled-task
# wrapper deletes them again at task start; deleting after the launch would
# race the file the running dispatch is writing).
rm -f "$LOG" "$DONE"

if [ "$IS_WIN" = "1" ]; then
  # Windows: Task Scheduler, not nohup (fleet.lane-launch). HOME and UTF-8 are
  # empty/CP950 in the scheduled-task environment; the lane script generated
  # above sets them explicitly before dispatching.
  command -v pwsh >/dev/null 2>&1 || { echo "review-pr.sh: pwsh not found" >&2; exit 1; }
  TASK="edda-review-pr$PR-r$ROUND"

  cat > "$SCRATCH/review-pr$PR-r$ROUND-launch.ps1" <<PS
foreach (\$f in @("$LOGW", "$DONEW")) { if (Test-Path \$f) { [System.IO.File]::Delete(\$f) } }
Unregister-ScheduledTask -TaskName '$TASK' -Confirm:\$false -ErrorAction SilentlyContinue
\$action = New-ScheduledTaskAction -Execute "pwsh.exe" -Argument "-NoProfile -NonInteractive -WindowStyle Hidden -ExecutionPolicy Bypass -File \`"$LANE_FILE_ARG\`"" -WorkingDirectory '$WTW'
\$settings = New-ScheduledTaskSettingsSet -ExecutionTimeLimit (New-TimeSpan -Minutes 30) -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries
Register-ScheduledTask -TaskName '$TASK' -Action \$action -Settings \$settings -RunLevel Limited | Out-Null
Start-ScheduledTask -TaskName '$TASK'
\$st = ""
for (\$i = 0; \$i -lt 20; \$i++) {
  Start-Sleep -Seconds 1
  \$st = (Get-ScheduledTask -TaskName '$TASK').State
  if (\$st -eq 'Running') { break }
}
\$info = Get-ScheduledTaskInfo -TaskName '$TASK'
"task=$TASK state=\$st lastTaskResult=\$(\$info.LastTaskResult)"
PS

  out=$(pwsh -NoProfile -ExecutionPolicy Bypass -File "$SCRATCH/review-pr$PR-r$ROUND-launch.ps1" 2>&1); rc=$?
  echo "$out"
  [ $rc -eq 0 ] || exit 1
  case "$out" in
    *state=Running*) : ;;
    *)
      # Task Scheduler says nothing useful about why. LastTaskResult=64 has now
      # been observed from three unrelated path faults on two machines, and the
      # lane script that everyone reads next was never reached, so name the one
      # thing only this process knows: whether the -File argument it generated
      # resolves for the pwsh that Task Scheduler starts (issue #683).
      echo "review-pr.sh: scheduled task did not reach Running" >&2
      if pwsh -NoProfile -NonInteractive -ExecutionPolicy Bypass \
           -Command "if (Test-Path -LiteralPath '$LANE_FILE_ARG') { exit 0 } else { exit 1 }" \
           >/dev/null 2>&1; then
        echo "review-pr.sh: the task's -File argument resolves for pwsh: $LANE_FILE_ARG — the fault is inside the lane script or its environment, not the path. Read $SCRATCH/review-pr$PR-r$ROUND.log" >&2
      else
        echo "review-pr.sh: the task's -File argument does NOT resolve for pwsh: $LANE_FILE_ARG — this is the LastTaskResult=64 failure of issue #683: the task registers and starts, then exits before writing any log or .done file. Check that \$EDDA_FLEET_SCRATCH is Windows-resolvable" >&2
      fi
      exit 1
      ;;
  esac
fi

if [ "$IS_WIN" = "0" ]; then
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

echo "log=$LOG"
echo "done=$DONE"
echo "session=$SID"
