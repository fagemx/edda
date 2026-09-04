#!/bin/sh
# pr-review-watch.sh — local watcher: for every open non-draft PR whose head SHA
# has not been reviewed yet, launch the read-only reviewer (scripts/review-pr.sh),
# post `review: started on <sha>` as an acknowledgement (retried, bounded), then
# post the SHA-pinned verdict when the reviewer finishes and set review:* labels.
#
# usage: pr-review-watch.sh [--once] [--dry-run]
#        pr-review-watch.sh decide                 (offline helper; TSV on stdin)
#        pr-review-watch.sh label-verdict <reviewed-sha> <current-head>
#        pr-review-watch.sh ack-try <pr> <sha> <attempts>
#
# Environment:
#   EDDA_REPO                  owner/repo            (default fagemx/edda)
#   EDDA_FLEET_ROOT            main checkout path    (default: derived from git)
#   EDDA_FLEET_SCRATCH         state/log dir         (default $HOME/.edda/fleet)
#   EDDA_REVIEW_MODEL          review model          (default claude-opus-5)
#   EDDA_REVIEW_POLL_SECONDS   poll interval         (default 60)
#   PR_REVIEW_WATCH_STATE      state file override   (used by tests)
#   PR_REVIEW_WATCH_ACKS       acks file override    (used by tests)
#
# State (under $EDDA_FLEET_SCRATCH, not in git):
#   review-state.tsv     pr<TAB>reviewed_sha<TAB>round
#   review-pending.tsv   pr<TAB>round<TAB>sha<TAB>attempts<TAB>launched_epoch<TAB>postfails
#   review-acks.tsv      pr<TAB>sha<TAB>attempts<TAB>status — heads launched but not
#                        yet acked; the entry exists while the ack is pending
#                        (removed on success). After 3 failed attempts the entry is
#                        RETAINED as a durable record: marked "post-failed" once the
#                        review:post-failed label is applied; if that label call also
#                        fails, the unmarked entry stays for the next poll.
#   review-fails.tsv     pr<TAB>sha<TAB>consecutive launch failures
#   watch.log            everything the watcher did
#
# Provider-overload rule, v1 (no Codex fallback — it cannot be made read-only,
# and it cannot reach Opus either; fleet.review-provider-overload=
# opus-default-sol-via-pi-fallback-no-codex, operator ruling 2026-09-03):
# on a dead/empty verdict, retry the review once through the same dispatch
# transport with the same model; if that also yields no verdict, label the PR
# `review:unreviewed`, log it, and stop for that head. Opus is the DEFAULT
# review engine, not the only one — the pool's anchor is still gpt-5.6-sol via
# pi — but the watcher itself never changes model: dropping to the anchor is an
# operator action, so the automated path stops at `review:unreviewed`.
# model_requested / model_observed / cost are read from the dispatch
# transcript — the `Model requested:` / `Model observed:` / `Cost:` lines
# `edda dispatch` prints — and are NEVER fabricated; a missing line is
# reported as unknown.
#
# review:unreviewed is per head: it blocks only while the PR's current head
# equals the head recorded as unreviewed. A new head drops the label and is
# reviewed again.
#
# The verdict comment is SHA-pinned; its label is applied only if the PR's
# current head still equals the reviewed SHA (a moved head gets the comment
# without the label, and the rescan launches the next round).
#
# A failed comment post or label update does NOT mark the head reviewed: the
# verdict is kept and posting retries on the next poll; after 5 failed
# attempts the PR is labeled `review:post-failed` (verdict file kept in
# $EDDA_FLEET_SCRATCH for manual posting).
#
# The watcher NEVER merges. Merge stays behind operator authorization
# (pr.merge-policy).
set -u

REPO=${EDDA_REPO:-fagemx/edda}
MODEL=${EDDA_REVIEW_MODEL:-claude-opus-5}
POLL=${EDDA_REVIEW_POLL_SECONDS:-60}
SCRATCH=${EDDA_FLEET_SCRATCH:-$HOME/.edda/fleet}
STALE=${EDDA_REVIEW_STALE_SECONDS:-2700}   # 45 min: the scheduled-task limit is 30 min
POSTFAIL_CAP=5
ACK_CAP=3

SCRIPT_DIR=$(cd "$(dirname "$0")" && pwd)
. "$SCRIPT_DIR/reviewer-capabilities.sh"
REVIEW_PR=${PR_REVIEW_WATCH_REVIEW_PR:-$SCRIPT_DIR/review-pr.sh}   # overridable for offline tests
LABEL_PR="$SCRIPT_DIR/review-pr.sh"   # always the real script (verdict-label is offline-safe)

ONCE=0
DRY=0
TAB=$(printf '\t')   # literal tab; "\t" inside quotes is backslash-t in POSIX sh
log() { echo "$(date -u '+%Y-%m-%dT%H:%M:%SZ') $*" >> "$WATCHLOG"; }

# ---- offline helpers (unit-tested by scripts/test-pr-review-watch.sh) --------

# decide: PR queue triage. Input: one TSV row per open non-draft PR from
#   gh pr list --repo R --state open --json number,headRefOid,isDraft,labels,updatedAt \
#     --jq '.[] | select(.isDraft|not) | [.number, .headRefOid, ([.labels[].name]|join(",")), .updatedAt] | @tsv'
# (row: number<TAB>sha<TAB>labels<TAB>updatedAt; drafts are filtered by the --jq).
# Output per PR: "REVIEW <n> <sha>" (+" drop-unreviewed-label" when a stale
# review:unreviewed label must be removed) or "SKIP <n> <reason>". Pure: no
# network, no state mutation; the state file is read-only here.
decide() {
  state=${PR_REVIEW_WATCH_STATE:-$SCRATCH/review-state.tsv}
  # Pass the state path via the environment, not -v: gawk processes escape
  # sequences in -v values, which mangles Windows paths (C:\Users\...).
  PR_REVIEW_STATE_TMP="$state" awk -F'\t' '
    BEGIN {
      sf = ENVIRON["PR_REVIEW_STATE_TMP"]
      if (sf != "") {
        while ((getline line < sf) > 0) {
          split(line, s, "\t")
          rev[s[1]] = s[2]
        }
        close(sf)
      }
    }
    {
      num = $1; sha = $2; labels = $3
      if (num !~ /^[0-9]+$/) next
      if (sha == "") { print "SKIP " num " missing-head"; next }
      if (("," labels ",") ~ /,review:unreviewed,/) {
        if (rev[num] == sha || rev[num] == "") {
          print "SKIP " num " review-unreviewed"; next
        }
        print "REVIEW " num " " sha " drop-unreviewed-label"; next
      }
      if (rev[num] == sha) { print "SKIP " num " already-reviewed"; next }
      print "REVIEW " num " " sha
    }
  '
}

# label-verdict: should the verdict label be applied? Only when the PR's
# current head still equals the reviewed SHA (and is known).
label_verdict() { # $1=reviewed sha  $2=current head
  if [ -n "${2:-}" ] && [ "$2" = "$1" ]; then echo apply; else echo skip; fi
}

# ---- acknowledgement (retried, bounded; never best-effort) -------------------

ack_register() { # $1=pr $2=sha $3=attempts $4=status(""=pending, "post-failed"=terminal)
  awk -F'\t' -v pr="$1" '$1!=pr' "$ACKS" > "$ACKS.tmp"
  printf '%s\t%s\t%s\t%s\n' "$1" "$2" "$3" "${4:-}" >> "$ACKS.tmp"
  mv "$ACKS.tmp" "$ACKS"
}

ack_drop() { # $1=pr
  awk -F'\t' -v pr="$1" '$1!=pr' "$ACKS" > "$ACKS.tmp"
  mv "$ACKS.tmp" "$ACKS"
}

ack_try() { # $1=pr $2=sha $3=attempts — one attempt.
  # exit 0 = posted (pending entry cleared); 1 = failed, retries remain;
  # 2 = failed after the bound. The entry is a durable record: after the bound
  # it is only marked "post-failed" once review:post-failed is APPLIED; if the
  # label call fails too, the entry stays for the next poll (no silent
  # terminal state).
  if printf 'review: started on %s\n' "$2" \
      | gh pr comment "$1" --repo "$REPO" --body-file - >/dev/null 2>&1; then
    ack_drop "$1"
    log "pr$1 ack posted: review: started on $2"
    return 0
  fi
  attempts=$((${3:-0} + 1))
  if [ "$attempts" -ge "$ACK_CAP" ]; then
    if gh pr edit "$1" --repo "$REPO" --add-label "review:post-failed" >/dev/null 2>&1; then
      ack_register "$1" "$2" "$attempts" "post-failed"
      log "pr$1 ack failed $ACK_CAP times — review:post-failed applied; ack entry marked post-failed"
    else
      ack_register "$1" "$2" "$attempts" ""
      log "pr$1 ack failed $ACK_CAP times AND review:post-failed label failed — ack entry kept for next poll"
    fi
    return 2
  fi
  ack_register "$1" "$2" "$attempts" ""
  log "pr$1 ack failed (attempt $attempts/$ACK_CAP); will retry next poll"
  return 1
}

# ---- offline subcommand dispatch (after all definitions) ---------------------
case "${1:-}" in
  decide) decide; exit 0 ;;
  label-verdict) label_verdict "${2:-}" "${3:-}"; exit 0 ;;
  ack-try)
    if [ $# -lt 4 ]; then echo "usage: pr-review-watch.sh ack-try <pr> <sha> <attempts>" >&2; exit 1; fi
    mkdir -p "$SCRATCH"
    ACKS=${PR_REVIEW_WATCH_ACKS:-$SCRATCH/review-acks.tsv}
    WATCHLOG=${PR_REVIEW_WATCH_LOG:-$SCRATCH/watch.log}
    touch "$ACKS"
    ack_try "$2" "$3" "$4"
    exit $?
    ;;
esac

for a in "$@"; do
  case "$a" in
    --once) ONCE=1 ;;
    --dry-run) DRY=1 ;;
    -h|--help) echo "usage: pr-review-watch.sh [--once] [--dry-run]"; exit 0 ;;
    *) echo "pr-review-watch.sh: unexpected argument '$a'" >&2; exit 1 ;;
  esac
done

mkdir -p "$SCRATCH"
STATE=${PR_REVIEW_WATCH_STATE:-$SCRATCH/review-state.tsv}
ACKS=${PR_REVIEW_WATCH_ACKS:-$SCRATCH/review-acks.tsv}
PENDING="$SCRATCH/review-pending.tsv"
FAILS="$SCRATCH/review-fails.tsv"
WATCHLOG=${PR_REVIEW_WATCH_LOG:-$SCRATCH/watch.log}
touch "$STATE" "$ACKS" "$PENDING" "$FAILS"


ensure_labels() {
  gh label create "review:lgtm"              --repo "$REPO" --color 0e8a16 --description "Automatic review verdict: LGTM (P0=0, P1=0)"            --force >/dev/null 2>&1 || true
  gh label create "review:changes-requested" --repo "$REPO" --color d93f0b --description "Automatic review verdict: Changes Requested"            --force >/dev/null 2>&1 || true
  gh label create "review:unreviewed"        --repo "$REPO" --color d4c5f9 --description "Review provider exhausted; verdict pending human/retry" --force >/dev/null 2>&1 || true
  gh label create "review:post-failed"       --repo "$REPO" --color b60205 --description "Verdict/ack could not be posted; see watcher scratch dir" --force >/dev/null 2>&1 || true
}

state_field() { # $1=pr  $2=field index (2=sha,3=round)
  awk -F'\t' -v pr="$1" -v f="$2" '$1==pr {print $f; found=1} END{if(!found) print ""}' "$STATE"
}

pending_has() { awk -F'\t' -v pr="$1" '$1==pr{found=1} END{exit !found}' "$PENDING" 2>/dev/null; }

pending_update() { # $1=pr $2=round $3=sha $4=attempts $5=launched $6=postfails
  awk -F'\t' -v pr="$1" '$1!=pr' "$PENDING" > "$PENDING.tmp"
  printf '%s\t%s\t%s\t%s\t%s\t%s\n' "$1" "$2" "$3" "$4" "$5" "$6" >> "$PENDING.tmp"
  mv "$PENDING.tmp" "$PENDING"
}

pending_drop() {
  awk -F'\t' -v pr="$1" '$1!=pr' "$PENDING" > "$PENDING.tmp"
  mv "$PENDING.tmp" "$PENDING"
}

state_set() { # $1=pr $2=sha $3=round
  awk -F'\t' -v pr="$1" '$1!=pr' "$STATE" > "$STATE.tmp"
  printf '%s\t%s\t%s\n' "$1" "$2" "$3" >> "$STATE.tmp"
  mv "$STATE.tmp" "$STATE"
}

fail_clear() { # $1=pr
  awk -F'\t' -v pr="$1" '$1!=pr' "$FAILS" > "$FAILS.tmp"
  mv "$FAILS.tmp" "$FAILS"
}

fail_bump() { # $1=pr $2=sha — exits 0 when the cap is reached
  c=$(awk -F'\t' -v pr="$1" '$1==pr{print $3; found=1} END{if(!found) print 0}' "$FAILS")
  c=$((c + 1))
  awk -F'\t' -v pr="$1" '$1!=pr' "$FAILS" > "$FAILS.tmp"
  printf '%s\t%s\t%s\n' "$1" "$2" "$c" >> "$FAILS.tmp"
  mv "$FAILS.tmp" "$FAILS"
  [ "$c" -ge 3 ]
}

# Extract the LAST verdict block between the fixed markers from a transcript.
extract_verdict() { # $1=log file, $2=out file
  tr -d '\r' < "$1" 2>/dev/null | awk '
    /<<<VERDICT/ {f=1; buf=""; next}
    /VERDICT>>>/ {if (f) {last=buf; f=0} next}
    f {buf = buf $0 "\n"}
    END {printf "%s", last}' > "$2"
  [ -s "$2" ]
}

verdict_ok() { # $1=verdict file
  grep -qE '^(LGTM|Changes Requested)' "$1" 2>/dev/null
}

# Model observed + cost, read from the transcript's receipt lines (edda
# dispatch prints them; the oversized-brief claude-stdin fallback writes the
# same lines) — never from what the brief asked for. Missing lines =>
# explicitly "unknown", never made up.
session_model_cost() { # $1=log file -> sets REQ, OBSERVED, COST, SIDO
  t=$(tr -d '\r' < "$1" 2>/dev/null)
  REQ=$(printf '%s\n' "$t" | sed -n 's/^Model requested: //p' | tail -1)
  OBSERVED=$(printf '%s\n' "$t" | sed -n 's/^Model observed: //p' | tail -1)
  COST=$(printf '%s\n' "$t" | sed -n 's/^Cost: \$\([0-9.]*\)$/\1/p' | tail -1)
  # `Session observed:` is the backend's OWN report of the conversation it ran
  # (dispatch prints `last_observed_session()` from claude's stream-json
  # `system/init`; the fallback arm prints claude's JSON `session_id`). The
  # `Session:` line above it is only the id edda asked for, so reading THAT one
  # here would compare the launched id with itself and never see a fork
  # (GH-708 round 1, P1). A reviewer that reported none stays "unknown".
  SIDO=$(printf '%s\n' "$t" | sed -n 's/^Session observed: //p' | tail -1)
  [ -n "${REQ:-}" ] || REQ="unknown"
  [ -n "${OBSERVED:-}" ] || OBSERVED="unknown"
  [ -n "${COST:-}" ] || COST="?"
  [ -n "${SIDO:-}" ] || SIDO="unknown"
}

# The reviewer conversation this round ran in, as a header fragment. Two
# independent facts are compared, never merged (GH-708): SESSION=/SESSION_MODE=
# are what review-pr.sh LAUNCHED with, and $2 is the id the backend REPORTED
# in-band (`Session observed:`), which is a different source, not an echo.
# Round 2+ of a PR is supposed to continue round 1's conversation, so a
# disagreement means the resume forked into a new one — printed as the defect
# it is instead of quietly showing the launched id. A reviewer that reported
# no observation at all leaves `$2` "unknown", which claims nothing.
reviewer_session_desc() { # $1=.done file  $2=session id observed in the log
  sid=$(sed -n 's/^SESSION=//p' "$1" 2>/dev/null | tail -1)
  mode=$(sed -n 's/^SESSION_MODE=//p' "$1" 2>/dev/null | tail -1)
  [ -n "${sid:-}" ] || sid=${2:-unknown}
  case "${mode:-}" in
    resume) mdesc=resumed ;;
    new)    mdesc=new ;;
    *)      mdesc='mode unknown — no SESSION_MODE receipt in .done' ;;
  esac
  if [ -n "${2:-}" ] && [ "$2" != "unknown" ] && [ -n "${sid:-}" ] && [ "$2" != "$sid" ]; then
    printf '`%s` (%s; BACKEND REPORTED `%s` — this round did not run in the launched conversation)' \
      "$sid" "$mdesc" "$2"
  else
    printf '`%s` (%s)' "$sid" "$mdesc"
  fi
}

pr_is_open() { # $1=pr
  [ "$(gh pr view "$1" --repo "$REPO" --json state --jq .state 2>/dev/null)" = "OPEN" ]
}

pr_head() { # $1=pr
  gh pr view "$1" --repo "$REPO" --json headRefOid --jq .headRefOid 2>/dev/null
}

# Trivial probe from the superseding fleet.review-provider-overload decision:
# before spending a full retry, check the review transport answers at minimal
# cost with the SAME --model (never a cheaper model). The transport is
# `edda dispatch --agent claude` (GH-708): pi's openrouter routing cannot
# reach any Anthropic model on this fleet.
PROBE_PROMPT="$SCRATCH/review-provider-probe.txt"
probe_review_provider() {
  review_capabilities edda-dispatch || return 2
  printf 'reply OK\n' > "$PROBE_PROMPT"
  if command -v timeout >/dev/null 2>&1; then
    timeout 120 edda dispatch --agent claude --model "$MODEL" --tools "$REVIEW_TOOLS" --exclude-tools "$REVIEW_DENIED" --prompt-file "$PROBE_PROMPT" >/dev/null 2>&1
  else
    edda dispatch --agent claude --model "$MODEL" --tools "$REVIEW_TOOLS" --exclude-tools "$REVIEW_DENIED" --prompt-file "$PROBE_PROMPT" >/dev/null 2>&1
  fi
}


process_acks() {
  [ -s "$ACKS" ] || return 0
  [ "$DRY" = "1" ] && return 0
  # NOTE: the redirect belongs on `done`, not on `read` — on `read` it re-opens
  # the file every iteration and a kept entry (unknown head, retrying ack)
  # spins forever.
  while IFS="$TAB" read -r pr sha attempts status; do
    [ -n "${pr:-}" ] || continue
    [ "${status:-}" = "post-failed" ] && continue   # terminal, already labeled
    ack_try "$pr" "$sha" "$attempts" || true
  done < "$ACKS"
}

mark_unreviewed() { # $1=pr $2=sha $3=round $4=reason
  log "pr$1 review exhausted: $4 — labeling review:unreviewed and stopping for head $2"
  if [ "$DRY" = "1" ]; then
    echo "dry-run: would label PR #$1 review:unreviewed ($4)"
    return 0
  fi
  gh pr edit "$1" --repo "$REPO" --add-label "review:unreviewed" >/dev/null 2>&1 || true
  state_set "$1" "$2" "$3"
}

mark_post_failed() { # $1=pr $2=sha $3=round $4=verdict file
  log "pr$1 posting failed $POSTFAIL_CAP times — labeling review:post-failed; verdict kept at $4"
  if [ "$DRY" = "1" ]; then
    echo "dry-run: would label PR #$1 review:post-failed"
    return 0
  fi
  gh pr edit "$1" --repo "$REPO" --add-label "review:post-failed" >/dev/null 2>&1 || true
  state_set "$1" "$2" "$3"
}

# ---- in-flight handling ------------------------------------------------------
settle_pending() {
  [ -s "$PENDING" ] || return 0
  now=$(date +%s)
  while IFS="$TAB" read -r pr round sha attempts launched postfails; do
    [ -n "${pr:-}" ] || continue

    if ! pr_is_open "$pr"; then
      log "pr$pr closed/merged while review in flight; dropping pending entry"
      pending_drop "$pr"
      continue
    fi

    DONE="$SCRATCH/review-pr$pr-r$round.done"
    LOG="$SCRATCH/review-pr$pr-r$round.log"
    VERDICT="$SCRATCH/review-pr$pr-r$round-verdict.md"
    POSTED="$VERDICT.posted"

    # Apply the verdict label + record reviewed. The label goes on only if the
    # PR's current head still equals the reviewed SHA; a moved head keeps the
    # SHA-pinned comment but gets no label, and the rescan launches the next
    # round.
    finish_verdict() { # $1=SRC verdict file (already posted)
      cur=$(pr_head "$pr")
      if [ -z "$cur" ]; then
        # A failed/empty head query is NOT evidence the head moved: keep the
        # verdict pending and retry on the next poll.
        log "pr$pr head unknown, retry — head query failed after posting the verdict; pending entry kept"
        return 0
      fi
      if [ "$(label_verdict "$sha" "$cur")" != "apply" ]; then
        log "pr$pr head moved: reviewed $sha current ${cur:-unknown} — verdict comment stays pinned, no label; rescan will launch the next round"
        state_set "$pr" "$sha" "$round"
        pending_drop "$pr"
        return 0
      fi
      vl=$(sh "$LABEL_PR" verdict-label < "$1")
      if [ -n "$vl" ] && gh pr edit "$pr" --repo "$REPO" --add-label "$vl" >/dev/null 2>&1; then
        gh pr edit "$pr" --repo "$REPO" --remove-label "review:unreviewed"  >/dev/null 2>&1 || true
        gh pr edit "$pr" --repo "$REPO" --remove-label "review:post-failed" >/dev/null 2>&1 || true
        if [ "$vl" = "review:lgtm" ]; then
          gh pr edit "$pr" --repo "$REPO" --remove-label "review:changes-requested" >/dev/null 2>&1 || true
        else
          gh pr edit "$pr" --repo "$REPO" --remove-label "review:lgtm" >/dev/null 2>&1 || true
        fi
        log "pr$pr r$round labeled $vl and recorded reviewed at $sha"
        state_set "$pr" "$sha" "$round"
        pending_drop "$pr"
      else
        postfails=$((postfails + 1))
        log "pr$pr r$round label update failed (attempt $postfails)"
        if [ "$postfails" -ge "$POSTFAIL_CAP" ]; then
          mark_post_failed "$pr" "$sha" "$round" "$1"
          pending_drop "$pr"
        else
          pending_update "$pr" "$round" "$sha" "$attempts" "$launched" "$postfails"
        fi
      fi
    }

    # Comment already posted; only the label + state remain.
    if [ -f "$POSTED" ]; then
      if [ "$DRY" = "1" ]; then
        echo "dry-run: would finish verdict posting for PR #$pr (label + state)"
        continue
      fi
      finish_verdict "$POSTED"
      continue
    fi

    is_done=0
    is_stale=0
    [ -f "$DONE" ] && is_done=1
    if [ "$is_done" = "0" ] && [ $((now - launched)) -gt "$STALE" ]; then
      is_stale=1
      log "pr$pr r$round review stale (${STALE}s, no .done); treating as dead"
    fi

    if [ "$is_done" = "1" ] || [ "$is_stale" = "1" ]; then
      if grep -q '^WORKTREE_CHECK=failed' "$DONE" 2>/dev/null; then
        mark_unreviewed "$pr" "$sha" "$round" 'review worktree changed; preserved for inspection'
        pending_drop "$pr"
        continue
      fi
      if extract_verdict "$LOG" "$VERDICT" && verdict_ok "$VERDICT"; then
        # A terminal receipt is one atomically published object, never an
        # incremental DISPATCH_EXIT line. Legacy receipts without FINAL_EXIT
        # and partial receipts fail
        # closed: source verification, worktree removal and task teardown must
        # all be proven before this watcher publishes a verdict.
        if ! grep -qx 'DISPATCH_EXIT=0' "$DONE" 2>/dev/null \
          || ! grep -qx 'FINAL_EXIT=0' "$DONE" 2>/dev/null \
          || ! grep -qx 'WORKTREE_CHECK=unchanged' "$DONE" 2>/dev/null \
          || ! grep -qx 'WORKTREE_CLEANUP=removed' "$DONE" 2>/dev/null \
          || ! grep -Eq '^TASK_CLEANUP=(unregistered|not-applicable)$' "$DONE" 2>/dev/null \
          || ! grep -qx 'TERMINAL_RECEIPT=complete' "$DONE" 2>/dev/null; then
          if [ $((now - launched)) -gt "$STALE" ]; then
            mark_unreviewed "$pr" "$sha" "$round" 'review has no clean atomic terminal receipt'
            pending_drop "$pr"
          else
            log "pr$pr r$round waiting for clean atomic terminal receipt"
          fi
          continue
        fi
        session_model_cost "$LOG"
        if [ "$COST" = "?" ]; then costline='cost: unknown'; else costline='cost: $'"$COST"; fi
        # The transport that actually ran, read from the lane's TRANSPORT=
        # receipt in .done — never the transport we wish had run (GH-708
        # round 2: this header used to hardcode `edda dispatch` even when the
        # oversized-brief claude-stdin fallback was the arm that ran).
        tline=$(sed -n 's/^TRANSPORT=//p' "$DONE" 2>/dev/null | tail -1)
        case "$tline" in
          edda-dispatch) tdesc='edda dispatch --agent claude' ;;
          claude-stdin)  tdesc='claude -p via stdin (oversized-brief fallback)' ;;
          *)             tdesc='unknown — no TRANSPORT receipt in .done' ;;
        esac
        tool_flags=$(sed -n 's/^TOOL_FLAGS=//p' "$DONE" 2>/dev/null | tail -1)
        tree_check=$(sed -n 's/^WORKTREE_CHECK=//p' "$DONE" 2>/dev/null | tail -1)
        [ -n "$tool_flags" ] || tool_flags='unknown — no TOOL_FLAGS receipt'
        [ -n "$tree_check" ] || tree_check='unknown — no WORKTREE_CHECK receipt'
        COMMENT="$SCRATCH/review-pr$pr-r$round-comment.md"
        {
          echo "> **Round $round review** — automatic watcher (\`scripts/pr-review-watch.sh\`): transport \`$tdesc\`, tool flags \`$tool_flags\`, worktree check \`$tree_check\`, model \`$REQ\` (observed \`$OBSERVED\`), reviewer_session $(reviewer_session_desc "$DONE" "$SIDO"), $costline, detached worktree at \`$sha\`. Reviewed head SHA: \`$sha\`."
          echo
          cat "$VERDICT"
        } > "$COMMENT"
        if [ "$DRY" = "1" ]; then
          echo "dry-run: would post $COMMENT to PR #$pr and set a review:* label"
          continue
        fi
        if gh pr comment "$pr" --repo "$REPO" --body-file "$COMMENT" >/dev/null 2>&1; then
          log "pr$pr r$round posted verdict comment"
          mv "$VERDICT" "$POSTED"
          pending_update "$pr" "$round" "$sha" "$attempts" "$launched" 0
          finish_verdict "$POSTED"
        else
          postfails=$((postfails + 1))
          log "pr$pr r$round comment post failed (attempt $postfails); verdict kept at $VERDICT"
          if [ "$postfails" -ge "$POSTFAIL_CAP" ]; then
            mark_post_failed "$pr" "$sha" "$round" "$VERDICT"
            pending_drop "$pr"
          else
            pending_update "$pr" "$round" "$sha" "$attempts" "$launched" "$postfails"
          fi
        fi
      else
        case "$attempts" in
          0)
            if [ "$DRY" = "1" ]; then
              echo "dry-run: would probe the provider and retry PR #$pr r$round via edda dispatch"
              pending_drop "$pr"
              continue
            fi
            if ! probe_review_provider; then
              mark_unreviewed "$pr" "$sha" "$round" "provider probe failed before the dispatch retry"
              pending_drop "$pr"
              continue
            fi
            log "pr$pr r$round provider probe OK — retrying once via edda dispatch (same model, fleet.review-provider-overload)"
            retry_out=$("$REVIEW_PR" "$pr" "$round" "$sha" --sha "$sha" 2>&1); retry_rc=$?
            printf '%s\n' "$retry_out" >> "$WATCHLOG"
            if [ "$retry_rc" = 0 ]; then
              actual_round=$(printf '%s\n' "$retry_out" | sed -n 's/^review_round=\([1-9][0-9]*\)$/\1/p' | tail -1)
              if [ -z "$actual_round" ]; then
                log "pr$pr retry returned no shared round receipt; keeping pending ownership"
                continue
              fi
              log "pr$pr r$actual_round dispatch retry launched"
              pending_update "$pr" "$actual_round" "$sha" 1 "$(date +%s)" "$postfails"
            else
              rc=$retry_rc
              if [ "$rc" = "3" ]; then
                log "pr$pr head moved before dispatch retry; dropping pending entry (rescan will re-review)"
              else
                log "pr$pr r$round dispatch retry launch failed (rc=$rc)"
              fi
              pending_drop "$pr"
            fi
            ;;
          *)
            mark_unreviewed "$pr" "$sha" "$round" "no verdict after one dispatch retry (v1 has no codex fallback)"
            pending_drop "$pr"
            ;;
        esac
      fi
    fi
  done < "$PENDING"
}

# ---- open-PR scan ------------------------------------------------------------
scan_open_prs() {
  rows=$(gh pr list --state open --repo "$REPO" \
      --json number,headRefOid,isDraft,labels,updatedAt \
      --jq '.[] | select(.isDraft|not) | [.number, .headRefOid, ([.labels[].name]|join(",")), .updatedAt] | @tsv' 2>/dev/null) || {
    log "gh pr list failed; skipping this cycle"
    return 0
  }
  decisions=$(printf '%s\n' "$rows" | decide) || {
    log "decide failed; skipping this cycle"
    return 0
  }
  printf '%s\n' "$decisions" | while read -r verb pr sha extra; do
    [ "${verb:-}" = "REVIEW" ] || continue
    [ -n "${pr:-}" ] || continue
    pending_has "$pr" && continue

    prev=$(state_field "$pr" 2)
    round=$(state_field "$pr" 3)
    [ -n "$round" ] || round=0
    newround=$((round + 1))

    if [ "$DRY" = "1" ]; then
      echo "dry-run: would launch review for PR #$pr round $newround (head $sha${prev:+, prev $prev}${extra:+, $extra}): $REVIEW_PR $pr $newround $prev --sha $sha"
      log "dry-run: would launch review for pr$pr r$newround (head $sha)"
      continue
    fi

    if [ "${extra:-}" = "drop-unreviewed-label" ]; then
      if gh pr edit "$pr" --repo "$REPO" --remove-label "review:unreviewed" >/dev/null 2>&1; then
        log "pr$pr new head $sha — stale review:unreviewed label (recorded for ${prev:-none}) removed"
      else
        log "pr$pr could not remove the stale review:unreviewed label (continuing)"
      fi
    fi

    log "pr$pr new head $sha (reviewed: '${prev:-none}') — launching review r$newround"
    out=$("$REVIEW_PR" "$pr" "$newround" "$prev" --sha "$sha" 2>&1); rc=$?
    [ -n "$out" ] && printf '%s\n' "$out" >> "$WATCHLOG"
    if [ "$rc" -eq 0 ]; then
      actual_round=$(printf '%s\n' "$out" | sed -n 's/^review_round=\([1-9][0-9]*\)$/\1/p' | tail -1)
      if [ -z "$actual_round" ]; then
        log "pr$pr launcher returned no shared round receipt; refusing to guess artifact paths"
        continue
      fi
      newround=$actual_round
      fail_clear "$pr"
      log "pr$pr r$newround launched and confirmed running (task edda-review-pr$pr-r$newround)"
      pending_update "$pr" "$newround" "$sha" 0 "$(date +%s)" 0
      ack_register "$pr" "$sha" 0
      log "pr$pr launched, ack pending (review: started on $sha)"
      ack_try "$pr" "$sha" 0 || true
    elif [ "$rc" = "3" ]; then
      log "pr$pr head moved between scan and launch ($sha); will rescan"
    else
      log "pr$pr r$newround launch failed (rc=$rc)"
      if fail_bump "$pr" "$sha"; then
        mark_unreviewed "$pr" "$sha" "$newround" "reviewer launch failed 3 times in a row"
        fail_clear "$pr"
      else
        log "pr$pr launch failure recorded; will retry next poll"
      fi
    fi
  done
}

cleanup() { log "watcher stopping (signal)"; exit 0; }
trap cleanup INT TERM

log "watcher starting (repo=$REPO poll=${POLL}s scratch=$SCRATCH once=$ONCE dry=$DRY)"
if [ "$DRY" != "1" ]; then
  ensure_labels
fi

while :; do
  process_acks
  settle_pending
  scan_open_prs
  [ "$ONCE" = "1" ] && break
  sleep "$POLL"
done
log "watcher cycle complete (once=$ONCE dry=$DRY)"
