#!/bin/sh
# review-pr.sh — launch one read-only review of a PR (claude-opus-5 via
# `edda dispatch --agent claude`), per fleet.lane-launch (Task Scheduler,
# hidden), fleet.review-brief-framing (validation-checklist brief) and
# fleet.review-engine-model / fleet.review-backend (the review model is fixed
# and explicitly pinned with --model; reviews run on the Claude subscription
# because pi's openrouter routing cannot reach any Anthropic model — GH-708).
#
# One reviewer conversation per PR, resumed across rounds
# (fleet.reviewer-agent=pi-with-per-pr-resumable-session, carried over to the
# claude transport by GH-708): the session id is derived from the PR number,
# so round 1 opens it with --session-id and every later round continues it
# with --resume and reads only the delta.
#
# The brief is BUILT BY READING REVIEW.md, the repo's executable review spec.
# This script contributes only the PR's facts (head SHA, surface, the linked
# issue's doneWhen); every review rule, severity, check command, the class
# router and the output format come from REVIEW.md, which is copied verbatim
# into the worktree as `.edda-review-spec.md` and pointed at by the brief. A
# missing REVIEW.md is a hard error — a brief without the spec would silently
# review against nothing. The spec is NOT inlined into the brief: inlining the
# ~33k-char spec pushed every real brief over the Windows 32767-char spawn cap
# that `edda dispatch` trips (os error 206), which silently switched every
# review to the fallback transport while every shipped claim said dispatch ran
# (GH-708 round 2, P1-1). Pointing at the file keeps the brief under the
# budget so the `edda dispatch` arm is the one that actually runs.
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
#                                  ("TRANSPORT=<arm>" naming the arm that
#                                  actually ran, "SESSION=<uuid>",
#                                  "SESSION_MODE=new|resume", then
#                                  "DISPATCH_EXIT=<code>")
#   wt-review-pr<N>/               detached worktree at the PR head, removed by
#                                  the lane once the round's verdict is in the
#                                  log and recreated at the same path next
#                                  round
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
. "$SELF_DIR/reviewer-capabilities.sh"
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

# Prefer the product-owned review command when the installed CLI exposes its
# complete, machine-readable contract. The legacy dispatch path below remains
# only for installations that cannot run that command yet.
product_review_supported() {
  product_help=$(edda review --help 2>&1) || return 1
  for product_flag in --pr --agent --model --json --resume; do
    printf '%s\n' "$product_help" | grep -qE -- "(^|[[:space:],])$product_flag([[:space:]=]|$)" || return 1
  done
}

launch_product_review() {
  # This branch owns its per-round artifacts, including on its first use.
  mkdir -p "$SCRATCH" || { echo "review-pr.sh: cannot create scratch directory $SCRATCH" >&2; exit 1; }
  LOG="$SCRATCH/review-pr$PR-r$ROUND.log"
  DONE="$SCRATCH/review-pr$PR-r$ROUND.done"
  LANE="$SCRATCH/review-pr$PR-r$ROUND-lane.ps1"
  RUNNER="$SCRATCH/review-pr$PR-r$ROUND-run.sh"
  PRODUCT_RESUME=""
  [ "$ROUND" -gt 1 ] && PRODUCT_RESUME="--resume"
  [ -n "$ROOT" ] || { echo "review-pr.sh: cannot locate main checkout (set EDDA_FLEET_ROOT)" >&2; exit 1; }

  if [ "$IS_WIN" = "1" ]; then
    command -v cygpath >/dev/null 2>&1 || { echo "review-pr.sh: cygpath not found" >&2; exit 1; }
    ROOTW=$(cygpath -w "$ROOT")
    LOGW=$(cygpath -w "$LOG")
    DONEW=$(cygpath -w "$DONE")
    LANEW=$(cygpath -w "$LANE")
    PWSH_EXE="pwsh.exe"
    command -v pwsh >/dev/null 2>&1 && PWSH_EXE=$(cygpath -w "$(command -v pwsh)")
    cat > "$LANE" <<PS
[Console]::OutputEncoding = [System.Text.UTF8Encoding]::new(\$false)
\$OutputEncoding = [System.Text.UTF8Encoding]::new(\$false)
Set-Location '$ROOTW'
function Invoke-EddaReview {
  & edda review --pr '$PR' --agent claude --model '$MODEL' --json $PRODUCT_RESUME
  return \$LASTEXITCODE
}
\$json = Invoke-EddaReview
\$code = \$LASTEXITCODE
\$payload = \$null
try { \$payload = \$json | ConvertFrom-Json } catch { }
\$proof = \$payload -and \$payload.subject.head_sha -eq '$SHA' -and \$payload.subject.subject_seen -eq '$SHA' -and \$payload.subject.worktree_check -eq 'unchanged' -and \$payload.reviewer.tool_policy -eq 'hard'
if (-not \$proof) {
  'edda review product receipt failed subject, internal-worktree, or hard-policy validation' | Out-File '$LOGW' -Encoding utf8
  \$json | Add-Content '$LOGW' -Encoding utf8
  \$code = 2
  \$tree = 'failed; product JSON lacked a matching internal worktree proof'
  \$policy = 'missing'
  \$session = 'unknown'
} else {
  \$p0 = @(\$payload.findings | Where-Object severity -eq 'P0').Count
  \$p1 = @(\$payload.findings | Where-Object severity -eq 'P1').Count
  \$qualified = \$payload.qualified -eq \$true -and @(\$payload.disqualifiers).Count -eq 0
  \$label = if (\$payload.verdict -eq 'lgtm' -and \$qualified) { 'LGTM' } elseif (\$payload.verdict -eq 'changes-requested') { 'Changes Requested' } else { '' }
  if (\$label) {
    "<<<VERDICT\`n### Verdict\`n\$label, P0=\$p0, P1=\$p1\`nEvent identity: \$(\$payload.event_id ?? 'unknown')\`nQualification: \$qualified\`nDisqualifiers: \$((@(\$payload.disqualifiers) -join ', ') ?? 'none')\`n### Findings" | Out-File '$LOGW' -Encoding utf8
    foreach (\$finding in @(\$payload.findings)) { Add-Content '$LOGW' ("finding: " + (\$finding | ConvertTo-Json -Compress)) -Encoding utf8 }
    Add-Content '$LOGW' '### Checklist' -Encoding utf8
    foreach (\$item in @(\$payload.checklist)) { Add-Content '$LOGW' ("checklist: " + (\$item | ConvertTo-Json -Compress)) -Encoding utf8 }
    Add-Content '$LOGW' '### Escalations' -Encoding utf8
    foreach (\$escalation in @(\$payload.escalations)) { Add-Content '$LOGW' ("escalation: " + (\$escalation | ConvertTo-Json -Compress)) -Encoding utf8 }
    Add-Content '$LOGW' 'VERDICT>>>' -Encoding utf8
  }
  Add-Content '$LOGW' "Model requested: \$(\$payload.reviewer.model_requested)" -Encoding utf8
  Add-Content '$LOGW' "Model observed: \$(\$payload.reviewer.model_observed)" -Encoding utf8
  if (\$null -ne \$payload.cost.usd) { Add-Content '$LOGW' ("Cost: \$" + [string]::Format([System.Globalization.CultureInfo]::InvariantCulture, '{0:0.####}', \$payload.cost.usd)) -Encoding utf8 }
  Add-Content '$LOGW' "Session: \$(\$payload.reviewer.session_id)" -Encoding utf8
  \$tree = 'unchanged'
  \$policy = 'product-json:hard'
  \$session = \$payload.reviewer.session_id
  \$disqualifiers = @(\$payload.disqualifiers) -join ','
}
"TRANSPORT=edda-review" | Out-File '$DONEW' -Encoding utf8
"POLICY_RECEIPT=\$policy" | Add-Content '$DONEW' -Encoding utf8
"SESSION=\$session" | Add-Content '$DONEW' -Encoding utf8
"SESSION_MODE=$( [ -n "$PRODUCT_RESUME" ] && echo resume || echo new )" | Add-Content '$DONEW' -Encoding utf8
"DISPATCH_EXIT=\$code" | Add-Content '$DONEW' -Encoding utf8
"WORKTREE_CHECK=\$tree" | Add-Content '$DONEW' -Encoding utf8
"QUALIFIED=\$qualified" | Add-Content '$DONEW' -Encoding utf8
"DISQUALIFIERS=\$disqualifiers" | Add-Content '$DONEW' -Encoding utf8
exit \$code
PS
    [ -s "$LANE" ] || { echo "review-pr.sh: Windows product lane generation produced no artifact" >&2; exit 1; }
    echo "lane_file_arg=$LANEW"
  else
    cat > "$RUNNER" <<RUN
#!/bin/sh
export HOME="\${HOME:-$(getent passwd "\$(id -u)" 2>/dev/null | cut -d: -f6)}"
cd '$ROOT' || exit 2
run_edda_review() { edda review --pr '$PR' --agent claude --model '$MODEL' --json $PRODUCT_RESUME; }
json='$LOG.json'
run_edda_review > "\$json" 2>&1
code=\$?
if ! command -v jq >/dev/null 2>&1 || ! jq -e --arg sha '$SHA' '.subject.head_sha == \$sha and .subject.subject_seen == \$sha and .subject.worktree_check == "unchanged" and .reviewer.tool_policy == "hard"' "\$json" >/dev/null 2>&1; then
  echo 'edda review product receipt failed subject, internal-worktree, or hard-policy validation' > '$LOG'
  cat "\$json" >> '$LOG'
  tree='failed; product JSON lacked a matching internal worktree proof'
  policy=missing
  session=unknown
  code=2
else
  verdict=\$(jq -r '.verdict' "\$json")
  p0=\$(jq '[.findings[] | select(.severity == "P0")] | length' "\$json")
  p1=\$(jq '[.findings[] | select(.severity == "P1")] | length' "\$json")
  qualified=\$(jq -r '.qualified == true and ((.disqualifiers // []) | length == 0)' "\$json")
  disqualifiers=\$(jq -r '(.disqualifiers // []) | join(",")' "\$json")
  case "\$verdict" in lgtm) label=LGTM;; changes-requested) label='Changes Requested';; *) label='';; esac
  [ "\$label" != LGTM ] || [ "\$qualified" = true ] || label=''
  : > '$LOG'
  if [ -n "\$label" ]; then
    printf '<<<VERDICT\n### Verdict\n%s, P0=%s, P1=%s\n' "\$label" "\$p0" "\$p1" >> '$LOG'
    jq -r '
      "Event identity: " + (.event_id // "unknown"),
      "Qualification: " + ((.qualified == true and ((.disqualifiers // []) | length == 0)) | tostring),
      "Disqualifiers: " + ((.disqualifiers // []) | if length == 0 then "none" else join(", ") end),
      "### Findings",
      (.findings[]? | "finding: " + tojson),
      "### Checklist",
      (.checklist[]? | "checklist: " + tojson),
      "### Escalations",
      (.escalations[]? | "escalation: " + tojson),
      "VERDICT>>>"
    ' "\$json" >> '$LOG'
  fi
  jq -r '"Model requested: " + .reviewer.model_requested, "Model observed: " + .reviewer.model_observed, (if .cost.usd == null then empty else "Cost: $" + (.cost.usd|tostring) end), "Session: " + .reviewer.session_id' "\$json" >> '$LOG'
  tree=unchanged
  policy=product-json:hard
  session=\$(jq -r '.reviewer.session_id' "\$json")
fi
printf 'TRANSPORT=edda-review\nPOLICY_RECEIPT=%s\nSESSION=%s\nSESSION_MODE=$( [ -n "$PRODUCT_RESUME" ] && echo resume || echo new )\nDISPATCH_EXIT=%s\nWORKTREE_CHECK=%s\nQUALIFIED=%s\nDISQUALIFIERS=%s\n' "\$policy" "\$session" "\$code" "\$tree" "\${qualified:-false}" "\${disqualifiers:-}" > '$DONE'
exit "\$code"
RUN
    sh -n "$RUNNER" || exit 1
    chmod +x "$RUNNER"
  fi
  if [ "$DRY" = "1" ]; then echo "dry-run: product review adapter generated; nothing launched."; exit 0; fi
  rm -f "$LOG" "$DONE"
  if [ "$IS_WIN" = "1" ]; then
    TASK="edda-review-pr$PR-r$ROUND"
    pwsh -NoProfile -Command "Unregister-ScheduledTask -TaskName '$TASK' -Confirm:\$false -ErrorAction SilentlyContinue; \$a=New-ScheduledTaskAction -Execute '$PWSH_EXE' -Argument '-NoProfile -NonInteractive -WindowStyle Hidden -ExecutionPolicy Bypass -File \`\"$LANEW\`\"' -WorkingDirectory '$ROOTW'; Register-ScheduledTask -TaskName '$TASK' -Action \$a -RunLevel Limited | Out-Null; Start-ScheduledTask -TaskName '$TASK'" || exit 1
  else
    nohup "$RUNNER" >/dev/null 2>&1 &
    pid=$!
    sleep 1
    kill -0 "$pid" 2>/dev/null || { echo "review-pr.sh: nohup process $pid died immediately" >&2; exit 1; }
    echo "task=nohup pid=$pid state=Running"
  fi
  echo "log=$LOG"; echo "done=$DONE"
  exit 0
}

if [ "${EDDA_REVIEW_PRODUCT_ADAPTER:-1}" != "0" ] && product_review_supported; then
  launch_product_review
fi

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
  # The base and head commits must be in this object database before they can be read.
  git -C "$SELF_REPO" cat-file -e "$BASE_SHA^{commit}" 2>/dev/null ||
    git -C "$SELF_REPO" fetch -q origin "$BASE_REF" 2>/dev/null || true
  git -C "$SELF_REPO" cat-file -e "$SHA^{commit}" 2>/dev/null ||
    git -C "$SELF_REPO" fetch -q origin "$BR" 2>/dev/null || true
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

# ---- reviewer session id: one resumable conversation per PR -----------------
# The claude backend (`claude -p`) accepts only valid UUIDs: the old
# `review-pr$PR` shape made every launch die with "Invalid session ID. Must be
# a valid UUID." (issue #694's second half, walked into by GH-708's transport
# switch).
#
# The id is DERIVED FROM THE PR NUMBER, not random, so every round of PR #N
# lands in the same reviewer conversation and round 2+ reads only the delta —
# the measured property `fleet.reviewer-agent=pi-with-per-pr-resumable-session`
# chose pi for, and which GH-708's transport switch must not give up. It is a
# name-based UUID (RFC 4122 §4.3 layout: SHA-1 digest, version nibble 5,
# variant 10xx) over the literal `edda-review-pr<N>`, so it is stable across
# machines, rounds and reruns, and can never collide with an implementer's
# session id (`review.independence-policy`).
review_session_uuid() { # $1 = PR number
  if command -v sha1sum >/dev/null 2>&1; then
    h=$(printf 'edda-review-pr%s' "$1" | sha1sum | cut -d' ' -f1)
  elif command -v shasum >/dev/null 2>&1; then
    h=$(printf 'edda-review-pr%s' "$1" | shasum -a 1 | cut -d' ' -f1)
  else
    echo "review-pr.sh: neither sha1sum nor shasum found — cannot derive the reviewer session UUID" >&2
    exit 1
  fi
  if [ "${#h}" -lt 32 ]; then
    echo "review-pr.sh: SHA-1 of the session name is too short ('$h')" >&2
    exit 1
  fi
  # Variant nibble: (digit & 0x3) | 0x8, i.e. the RFC's 10xx bits.
  case $(printf '%s' "$h" | cut -c17) in
    0|4|8|c) v=8 ;;
    1|5|9|d) v=9 ;;
    2|6|a|e) v=a ;;
    3|7|b|f) v=b ;;
    *) echo "review-pr.sh: SHA-1 digest is not hex ('$h')" >&2; exit 1 ;;
  esac
  printf '%s-%s-5%s-%s%s-%s\n' \
    "$(printf '%s' "$h" | cut -c1-8)" \
    "$(printf '%s' "$h" | cut -c9-12)" \
    "$(printf '%s' "$h" | cut -c14-16)" \
    "$v" \
    "$(printf '%s' "$h" | cut -c18-20)" \
    "$(printf '%s' "$h" | cut -c21-32)"
}
SID=$(review_session_uuid "$PR") || exit 1

# New conversation or continued one? Claude Code refuses a `--session-id` that
# already exists ("Session ID <id> is already in use", exit 1), so the two are
# different flags and the choice must be made from what is actually on disk —
# not from the round number, which is 2 both when round 1 produced a session
# and when round 1 died before creating one. The session store is keyed by the
# launch cwd, so the glob spans every project dir; resume itself is
# cwd-independent (verified GH-708: a `--resume` from an unrelated directory
# still continued the conversation and kept appending to the SAME transcript),
# which is why deleting and recreating the per-PR worktree between rounds does
# not break it.
SESSION_MODE=new
for f in "$HOME/.claude/projects"/*/"$SID.jsonl"; do
  if [ -f "$f" ]; then SESSION_MODE=resume; break; fi
done

# ---- brief: PR facts + REVIEW.md verbatim (fleet.review-brief-framing) ------
BRIEF="$SCRATCH/review-pr$PR-r$ROUND-brief.md"
{
  echo "# Review brief — PR #$PR, Round $ROUND (read-only, $MODEL_SHORT, session $SID)"
  echo
  echo "You stand in a detached worktree at PR head **$SHA** (branch \`$BR\`). The workspace ledger is reachable (\`edda ask\` works). Title: $TITLE. Issue(s): ${ISSUES:-none}. Files changed by the PR: ${SURFACE:-none}"
  if [ -n "$PREV" ]; then
    echo "This is a DELTA round: your prior verdict on $PREV is posted on the PR; review \`git diff $PREV..$SHA\` and RAN-confirm each prior finding is resolved; do not re-review the whole PR."
    if [ "$SESSION_MODE" = "resume" ]; then
      echo "You are the SAME reviewer session that produced it (\`$SID\`, resumed), so your earlier rounds are above this message — read the delta, not the PR."
    else
      echo "NOTE: the reviewer session \`$SID\` carries no prior transcript, so the earlier rounds are NOT in your context — read your posted verdict on $PREV from the PR before judging the delta."
    fi
  fi
  echo
  echo "## The spec you run"
  echo "This review is **REVIEW.md ($SPEC_VERSION)**. The launcher has copied it verbatim to **\`.edda-review-spec.md\`** at the worktree root (your cwd) — an untracked helper file written by the launcher, not part of the PR. Read that file FIRST, in full, and run it top to bottom. It is the only source of review rules, severities, check commands and output format; everything else in this brief is only this PR's facts."
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
  echo "### Wiring and write-end swallow scan (scripts/wiring-scan.sh)"
  echo "Reviewer aid: scan for swallow patterns on added lines (\`let _ =\`, \`.ok();\`, \`unwrap_or_default()\`, \`best-effort\`, \`silently\`):"
  echo '```'
  swallow_scan=$(sh "$SELF_REPO/scripts/wiring-scan.sh" "$BASE_SHA" "$SHA" 2>/dev/null)
  scan_rc=$?
  swallow_lens=""
  if [ "$scan_rc" -eq 0 ] && [ -n "$swallow_scan" ]; then
    swallow_lens=$(printf '%s\n' "$swallow_scan" | awk '/^== Swallow patterns/,0')
  fi
  if [ -n "$swallow_lens" ]; then
    printf '%s\n' "$swallow_lens"
  else
    echo "(wiring scan unavailable)"
  fi
  echo '```'
  echo "A write-end swallow on a path where coordination, ledger, heartbeat, session-ledger, L3-store, or digest state is written is **P1** (REVIEW.md §5.5, GH-692, GH-733)."
  echo
  echo "## Output"
  echo "Print the REVIEW.md §7 verdict — every field, the Rules table with one row per routed rule, the Wiring table — between the markers below, with this header line filled in:"
  echo "<<<VERDICT"
  echo "## Code Review: Round $ROUND — PR #$PR @ $SHA"
  echo "…the rest exactly as REVIEW.md §7 specifies (model_requested: $MODEL, spec: $SPEC_VERSION, class: $CANON_CLASS)…"
  echo "VERDICT>>>"
  echo
  echo "The REVIEW.md §7 header specifies \`reviewer_session: $SID\` (this round: **$SESSION_MODE**) directly under \`model_observed\` (GH-708, GH-756)."
  echo
  echo "---"
  echo
  echo "(End of brief — the spec itself is at \`.edda-review-spec.md\` in the worktree root, from $SPEC_SOURCE.)"
} > "$BRIEF"

echo "brief=$BRIEF"
echo "sha=$SHA"
echo "issues=${ISSUES:-none}"
echo "surface=${SURFACE:-none}"
echo "classes=${CLASSES:-code-plain}"
echo "canonical_class=$CANON_CLASS"
echo "spec=$SPEC_SOURCE ($SPEC_VERSION)"
echo "base=$BASE_SHA"
echo "session=$SID"
echo "session_mode=$SESSION_MODE"

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

# The two arms spell "continue this conversation" differently: `edda dispatch`
# keeps --session-id and adds --resume (which requires it), while `claude -p`
# swaps --session-id for --resume — there they are mutually exclusive, and a
# repeated --session-id is a hard error rather than a resume (GH-708).
DISPATCH_SESSION_ARGS="--session-id '$SID'"
CLAUDE_SESSION_ARGS="--session-id '$SID'"
if [ "$SESSION_MODE" = "resume" ]; then
  DISPATCH_SESSION_ARGS="--session-id '$SID' --resume"
  CLAUDE_SESSION_ARGS="--resume '$SID'"
fi
if [ "$IS_WIN" = "1" ]; then
  command -v cygpath >/dev/null 2>&1 || { echo "review-pr.sh: cygpath not found" >&2; exit 1; }
  WTW=$(cygpath -w "$WT")
  BRIEFW=$(cygpath -w "$BRIEF")
  LOGW=$(cygpath -w "$LOG")
  DONEW=$(cygpath -w "$DONE")
  ERRW=$(cygpath -w "$DONE.err")
  LANE_FILE_ARG=$(cygpath -w "$LANE")
  SCRATCHW=$(cygpath -w "$SCRATCH")
  # Empty only when the main checkout could not be located; the launch path
  # refuses that case below, and a dry-run never runs the lane.
  ROOTW=""
  [ -n "$ROOT" ] && ROOTW=$(cygpath -w "$ROOT")
  CAPSW=$(cygpath -w "$SELF_DIR/fleet/reviewer-capabilities.ps1")
  # Resolve pwsh to a full Windows path for the task action: on a Store (MSIX)
  # install the bare "pwsh.exe" execution alias does not launch under Task
  # Scheduler — the task dies with LastTaskResult=0x80070002 before the lane
  # runs. Optional so offline dry-runs on machines without pwsh still work.
  PWSH_EXE="pwsh.exe"
  if command -v pwsh >/dev/null 2>&1; then
    PWSH_EXE=$(cygpath -w "$(command -v pwsh)")
  fi
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
. '$CAPSW'
function Get-ReviewWorktreeSnapshot {
  \$entries = [System.Collections.Generic.List[string]]::new()
  foreach (\$scope in @(
    @{ Name = 'tracked'; Args = @('--cached') },
    @{ Name = 'untracked'; Args = @('--others', '--exclude-standard') },
    @{ Name = 'ignored'; Args = @('--others', '--ignored', '--exclude-standard') }
  )) {
    \$paths = & git ls-files @(\$scope.Args)
    if (\$LASTEXITCODE -ne 0) { throw "git ls-files failed for \$(\$scope.Name) scope" }
    foreach (\$path in \$paths) {
      if (Test-Path -LiteralPath \$path -PathType Leaf) {
        \$hash = (& git hash-object -- \$path)
        if (\$LASTEXITCODE -ne 0) { throw "git hash-object failed for \$(\$scope.Name) path \$path" }
        [void]\$entries.Add("\$(\$scope.Name)\`t\$hash\`t\$path")
      } elseif (Test-Path -LiteralPath \$path -PathType Container) {
        [void]\$entries.Add("\$(\$scope.Name)\`tdirectory\`t\$path")
      } else {
        [void]\$entries.Add("\$(\$scope.Name)\`tmissing-or-special\`t\$path")
      }
    }
  }
  \$bytes = [System.Text.Encoding]::UTF8.GetBytes((\$entries -join "\`n"))
  return [Convert]::ToHexString([System.Security.Cryptography.SHA256]::HashData(\$bytes)).ToLowerInvariant()
}
\$beforeStatus = (& git status --porcelain=v1 --untracked-files=all) -join "\n"
if (\$LASTEXITCODE -ne 0) { 'DISPATCH_EXIT=2' | Out-File '$DONEW'; exit 2 }
try { \$beforeSnapshot = Get-ReviewWorktreeSnapshot } catch { \$_ | Out-File '$DONEW'; 'DISPATCH_EXIT=2' | Out-File '$DONEW' -Append; exit 2 }
function Test-ReviewWorktree {
  \$afterHead = & git rev-parse HEAD
  if (\$LASTEXITCODE -ne 0) { 'WORKTREE_CHECK=failed; git HEAD unavailable' | Out-File '$DONEW' -Append; return \$false }
  \$afterStatus = (& git status --porcelain=v1 --untracked-files=all) -join "\n"
  if (\$LASTEXITCODE -ne 0) { 'WORKTREE_CHECK=failed; git status unavailable' | Out-File '$DONEW' -Append; return \$false }
  try { \$afterSnapshot = Get-ReviewWorktreeSnapshot } catch { \$_ | Out-File '$DONEW' -Append; 'WORKTREE_CHECK=failed; source snapshot unavailable' | Out-File '$DONEW' -Append; return \$false }
  & git status --short | Out-File '$LOGW' -Append -Encoding utf8
  & git log -1 --format=%H | Out-File '$LOGW' -Append -Encoding utf8
  if (\$LASTEXITCODE -ne 0 -or \$afterHead -ne '$SHA' -or \$afterStatus -ne \$beforeStatus -or \$afterSnapshot -ne \$beforeSnapshot) {
    'WORKTREE_CHECK=failed; preserved for inspection' | Out-File '$DONEW' -Append -Encoding utf8
    return \$false
  }
  'WORKTREE_CHECK=unchanged' | Out-File '$DONEW' -Append -Encoding utf8
  return \$true
}
function Remove-ReviewWorktree {
  # The verdict is in the log by the time this runs, so the detached worktree
  # has served its purpose. Left behind, they accumulate: 14 stale
  # wt-review-pr* trees sat on this workstation when GH-708 was written. The
  # path stays per-PR and is recreated at the same place next round, which
  # does not break --resume (verified: resume is cwd-independent and keeps
  # appending to the same transcript). --force because the launcher copies
  # the untracked .edda-review-spec.md into the tree. A failure here (a file
  # still locked, say) must not change the round's exit code.
  if ('$ROOTW' -eq '') { return }
  Set-Location '$SCRATCHW'
  & git -C '$ROOTW' worktree remove --force '$WTW' 2>&1 | Out-Null
}
# edda dispatch hands the prompt to 'claude -p' as a command-line argument,
# and Windows caps a command line at 32767 chars — an oversized prompt dies in
# the spawn with os error 206 before claude starts. The brief stays far under
# that budget because the spec is referenced (.edda-review-spec.md in the
# worktree), not inlined; the guard is the honest safety valve for a brief
# that grows past it anyway (a giant doneWhen, say). The fallback runs the
# recorded fleet.review-engine-model shape — the brief piped to claude via
# stdin with the same restricted --tools capability allowlist —
# never an unrestricted reviewer (GH-708 round 2, P1-2). Either arm writes a
# TRANSPORT= receipt into .done, and the verdict header the watcher posts
# prints that receipt verbatim instead of a hardcoded transport.
\$briefChars = (Get-Content -Raw "$BRIEFW").Length
if (\$briefChars -lt 30000) {
  try { Assert-ReviewCapabilities 'edda-dispatch' } catch { \$_ | Out-File '$LOGW'; 'DISPATCH_EXIT=2' | Out-File '$DONEW'; exit 2 }
  & edda dispatch --agent claude --model '$MODEL' --permission-mode '$REVIEW_PERMISSION_MODE' --tools '$REVIEW_TOOLS' --exclude-tools '$REVIEW_DENIED' $DISPATCH_SESSION_ARGS --prompt-file "$BRIEFW" 2>&1 | Out-File -FilePath "$LOGW" -Encoding utf8
  \$code = \$LASTEXITCODE
  "TRANSPORT=edda-dispatch" | Out-File "$DONEW" -Encoding utf8
  "TOOL_FLAGS=--permission-mode '$REVIEW_PERMISSION_MODE' --tools '$REVIEW_TOOLS' --exclude-tools '$REVIEW_DENIED'" | Out-File "$DONEW" -Append -Encoding utf8
  "SESSION=$SID" | Out-File "$DONEW" -Append -Encoding utf8
  "SESSION_MODE=$SESSION_MODE" | Out-File "$DONEW" -Append -Encoding utf8
  "DISPATCH_EXIT=\$code" | Out-File "$DONEW" -Append -Encoding utf8
  if (Test-ReviewWorktree) { Remove-ReviewWorktree } else { exit 2 }
  exit \$code
}
\$raw = Get-Content -Raw "$BRIEFW"
try { Assert-ReviewCapabilities 'claude-stdin' } catch { \$_ | Out-File '$LOGW'; 'DISPATCH_EXIT=2' | Out-File '$DONEW'; exit 2 }
\$json = \$raw | & claude -p --model '$MODEL' --output-format json --permission-mode '$REVIEW_PERMISSION_MODE' $CLAUDE_SESSION_ARGS --tools '$REVIEW_TOOLS' --disallowedTools '$REVIEW_DENIED' 2>"$ERRW"
\$code = \$LASTEXITCODE
\$r = \$null
try { \$r = \$json | ConvertFrom-Json } catch { }
if (\$r -and \$r.result) {
  \$r.result | Out-File -FilePath "$LOGW" -Encoding utf8
  \$mu = @(\$r.modelUsage.PSObject.Properties.Name) | Select-Object -First 1
  if (-not \$mu) { \$mu = 'unknown' }
  Add-Content -Path "$LOGW" -Value 'Model requested: $MODEL' -Encoding utf8
  Add-Content -Path "$LOGW" -Value "Model observed: \$mu" -Encoding utf8
  if (\$null -ne \$r.total_cost_usd) {
    # InvariantCulture: the default conversion renders a comma-decimal locale's
    # "0,33", which the watcher's [0-9.]* pattern drops to cost: unknown.
    Add-Content -Path "$LOGW" -Value ("Cost: \$" + [math]::Round(\$r.total_cost_usd, 2).ToString([System.Globalization.CultureInfo]::InvariantCulture)) -Encoding utf8
  }
  Add-Content -Path "$LOGW" -Value "Session: $SID" -Encoding utf8
  Add-Content -Path "$LOGW" -Value "Session observed: \$(\$r.session_id)" -Encoding utf8
} else {
  \$json | Out-File -FilePath "$LOGW" -Encoding utf8
}
if (Test-Path "$ERRW") { Get-Content "$ERRW" | Add-Content -Path "$LOGW" -Encoding utf8; Remove-Item "$ERRW" -ErrorAction SilentlyContinue }
"TRANSPORT=claude-stdin" | Out-File "$DONEW" -Encoding utf8
"TOOL_FLAGS=--permission-mode '$REVIEW_PERMISSION_MODE' --tools '$REVIEW_TOOLS' --disallowedTools '$REVIEW_DENIED'" | Out-File "$DONEW" -Append -Encoding utf8
"SESSION=$SID" | Out-File "$DONEW" -Append -Encoding utf8
"SESSION_MODE=$SESSION_MODE" | Out-File "$DONEW" -Append -Encoding utf8
"DISPATCH_EXIT=\$code" | Out-File "$DONEW" -Append -Encoding utf8
if (Test-ReviewWorktree) { Remove-ReviewWorktree } else { exit 2 }
exit \$code
PS
else
  # Linux: no job-object trap, plain nohup is enough. The source snapshot
  # consumes Git's NUL-delimited path inventory, so this runner requires Bash
  # rather than silently parsing it with POSIX `read`.
  cat > "$RUNNER" <<RUN
#!/usr/bin/env bash
set -o pipefail
export HOME="\${HOME:-$(getent passwd "\$(id -u)" 2>/dev/null | cut -d: -f6)}"
cd '$WT' || exit 1
. '$SELF_DIR/reviewer-capabilities.sh'
before_snapshot_file='$DONE.before-snapshot'
after_snapshot_file='$DONE.after-snapshot'
cleanup_review_snapshots() {
  rm -f -- "\$before_snapshot_file" "\$after_snapshot_file"
}
trap cleanup_review_snapshots EXIT
trap 'cleanup_review_snapshots; exit 130' HUP INT TERM
review_hash_scope() {
  scope=\$1
  shift
  # Bash's read -d keeps Git's NUL inventory intact, including newline paths.
  # pipefail makes a failed git ls-files command fail this snapshot too.
  git ls-files -z "\$@" | while IFS= read -r -d '' path; do
    if [ -f "\$path" ]; then
      hash=\$(git hash-object -- "\$path") || exit 1
      printf '%s\t%s\t%s\0' "\$scope" "\$hash" "\$path"
    elif [ -L "\$path" ]; then
      printf '%s\tsymlink\t%s\t%s\0' "\$scope" "\$(readlink "\$path")" "\$path"
    else
      printf '%s\tmissing-or-special\t%s\0' "\$scope" "\$path"
    fi
  done
}
review_worktree_snapshot() {
  snapshot_file=\$1
  : > "\$snapshot_file" || return 1
  review_hash_scope tracked --cached > "\$snapshot_file" || return 1
  review_hash_scope untracked --others --exclude-standard >> "\$snapshot_file" || return 1
  review_hash_scope ignored --others --ignored --exclude-standard >> "\$snapshot_file" || return 1
  git hash-object --stdin < "\$snapshot_file"
}
before_status=\$(git status --porcelain=v1 --untracked-files=all) || exit 2
before_snapshot=\$(review_worktree_snapshot "\$before_snapshot_file") || { echo 'DISPATCH_EXIT=2' > '$DONE'; exit 2; }
check_review_worktree() {
  after_head=\$(git rev-parse HEAD) || { echo 'WORKTREE_CHECK=failed; git HEAD unavailable' >> '$DONE'; return 1; }
  after_status=\$(git status --porcelain=v1 --untracked-files=all) || { echo 'WORKTREE_CHECK=failed; git status unavailable' >> '$DONE'; return 1; }
  after_snapshot=\$(review_worktree_snapshot "\$after_snapshot_file") || { echo 'WORKTREE_CHECK=failed; source snapshot unavailable' >> '$DONE'; return 1; }
  git status --short >> '$LOG'
  git log -1 --format=%H >> '$LOG'
  if [ "\$after_head" != '$SHA' ] || [ "\$after_status" != "\$before_status" ] || [ "\$after_snapshot" != "\$before_snapshot" ]; then
    echo 'WORKTREE_CHECK=failed; preserved for inspection' >> '$DONE'
    return 1
  fi
  echo 'WORKTREE_CHECK=unchanged' >> '$DONE'
}
# Same lifecycle as the Windows lane's Remove-ReviewWorktree (see there): the
# verdict is in the log once the reviewer exits, so the detached worktree is
# removed rather than left to accumulate. Never changes the round's exit code.
remove_review_worktree() {
  [ -n '$ROOT' ] || return 0
  cd '$SCRATCH' || return 0
  git -C '$ROOT' worktree remove --force '$WT' >/dev/null 2>&1 || true
}
# Same size guard as the Windows lane (see there): edda dispatch while the
# brief fits the Windows-shaped spawn budget, the read-only-allowlisted
# claude-via-stdin fallback above it. TRANSPORT= names the arm that ran.
chars=\$(wc -m < '$BRIEF')
if [ "\$chars" -lt 30000 ]; then
  review_capabilities edda-dispatch > '$LOG' 2>&1 || { echo 'DISPATCH_EXIT=2' > '$DONE'; exit 2; }
  edda dispatch --agent claude --model '$MODEL' --permission-mode '$REVIEW_PERMISSION_MODE' --tools '$REVIEW_TOOLS' --exclude-tools '$REVIEW_DENIED' $DISPATCH_SESSION_ARGS --prompt-file '$BRIEF' > '$LOG' 2>&1
  code=\$?
  echo "TRANSPORT=edda-dispatch" > '$DONE'
  echo "TOOL_FLAGS=--permission-mode '$REVIEW_PERMISSION_MODE' --tools '$REVIEW_TOOLS' --exclude-tools '$REVIEW_DENIED'" >> '$DONE'
  echo "SESSION=$SID" >> '$DONE'
  echo "SESSION_MODE=$SESSION_MODE" >> '$DONE'
  echo "DISPATCH_EXIT=\$code" >> '$DONE'
  check_review_worktree && remove_review_worktree || exit 2
  exit \$code
fi
review_capabilities claude-stdin > '$LOG' 2>&1 || { echo 'DISPATCH_EXIT=2' > '$DONE'; exit 2; }
claude -p --model '$MODEL' --output-format json --permission-mode '$REVIEW_PERMISSION_MODE' $CLAUDE_SESSION_ARGS --tools '$REVIEW_TOOLS' --disallowedTools '$REVIEW_DENIED' < '$BRIEF' > '$LOG.json' 2>'$LOG.err'
code=\$?
if command -v jq >/dev/null 2>&1; then
  jq -r '.result // empty' '$LOG.json' > '$LOG'
  mu=\$(jq -r '.modelUsage | keys[0] // "unknown"' '$LOG.json')
  printf 'Model requested: %s\n' '$MODEL' >> '$LOG'
  printf 'Model observed: %s\n' "\$mu" >> '$LOG'
  jq -r 'if .total_cost_usd != null then "Cost: \$" + ((.total_cost_usd * 100 | round) / 100 | tostring) else empty end' '$LOG.json' >> '$LOG'
  printf 'Session: %s
' '$SID' >> '$LOG'
  jq -r '"Session observed: " + (.session_id // "unknown")' '$LOG.json' >> '$LOG'
else
  # No jq is not a silent degradation into a dead verdict: name the fault in
  # the log the watcher reads, keep the raw JSON for diagnosis, and fail the
  # round explicitly (DISPATCH_EXIT=1) rather than letting extract_verdict
  # find no markers and label the head review:unreviewed with no reason.
  echo "review-pr runner: jq not found on PATH — cannot extract the verdict from claude's JSON output; install jq and relaunch" > '$LOG'
  cat '$LOG.json' >> '$LOG'
  code=1
fi
[ -s '$LOG' ] || cp '$LOG.json' '$LOG'
[ -f '$LOG.err' ] && cat '$LOG.err' >> '$LOG'
rm -f '$LOG.json' '$LOG.err'
echo "TRANSPORT=claude-stdin" > '$DONE'
echo "TOOL_FLAGS=--permission-mode '$REVIEW_PERMISSION_MODE' --tools '$REVIEW_TOOLS' --disallowedTools '$REVIEW_DENIED'" >> '$DONE'
echo "SESSION=$SID" >> '$DONE'
echo "SESSION_MODE=$SESSION_MODE" >> '$DONE'
echo "DISPATCH_EXIT=\$code" >> '$DONE'
check_review_worktree && remove_review_worktree || exit 2
exit \$code
RUN
  command -v bash >/dev/null 2>&1 || { echo "review-pr.sh: bash is required for the Linux NUL-safe source snapshot" >&2; exit 1; }
  bash -n "$RUNNER" || exit 1
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
# Reject before creating the worktree or scheduling a job, then recheck in the
# generated lane because its PATH (and installed binary) may differ.
if [ "$(wc -m < "$BRIEF")" -lt 30000 ]; then
  review_capabilities edda-dispatch || exit 2
else
  review_capabilities claude-stdin || exit 2
fi

# ---- detached worktree at the PR head ---------------------------------------
# Prune first: the lane removes its worktree when the round ends, so a
# registration whose directory is gone is either that removal (recorded but
# not yet pruned) or a tree deleted by hand. Either way `worktree add` at the
# same per-PR path would refuse it as "already registered" (GH-708).
git -C "$ROOT" worktree prune
git -C "$ROOT" fetch -q origin "$BR"
if [ -d "$WT" ]; then
  git -C "$WT" checkout -q --detach "$SHA"
else
  git -C "$ROOT" worktree add --detach "$WT" "$SHA" >/dev/null
fi

# The detached worktree is the reviewer's cwd, so the spec copy must live
# INSIDE it: a scratch path would be an out-of-cwd read for claude. Written
# fresh every launch (including onto an existing worktree after a checkout)
# so the brief's .edda-review-spec.md pointer is always the base SHA's spec.
cp "$SPEC" "$WT/.edda-review-spec.md"

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
foreach (\$f in @("$LOGW", "$DONEW", "$DONEW.err")) { if (Test-Path \$f) { [System.IO.File]::Delete(\$f) } }
Unregister-ScheduledTask -TaskName '$TASK' -Confirm:\$false -ErrorAction SilentlyContinue
\$action = New-ScheduledTaskAction -Execute "$PWSH_EXE" -Argument "-NoProfile -NonInteractive -WindowStyle Hidden -ExecutionPolicy Bypass -File \`"$LANE_FILE_ARG\`"" -WorkingDirectory '$WTW'
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
