#!/bin/sh
# pr-review-watch.sh — local watcher: for every open non-draft PR whose head SHA
# has not been reviewed yet, launch the read-only reviewer (scripts/review-pr.sh),
# post the SHA-pinned verdict as a PR comment, and set review:* labels.
#
# usage: pr-review-watch.sh [--once] [--dry-run]
#
# Environment:
#   EDDA_REPO                  owner/repo            (default fagemx/edda)
#   EDDA_FLEET_ROOT            main checkout path    (default: derived from git)
#   EDDA_FLEET_SCRATCH         state/log dir         (default $HOME/.edda/fleet)
#   EDDA_REVIEW_MODEL          review model          (default openai-codex/gpt-5.6-sol)
#   EDDA_REVIEW_POLL_SECONDS   poll interval         (default 60)
#
# State (under $EDDA_FLEET_SCRATCH, not in git):
#   review-state.tsv     pr<TAB>reviewed_sha<TAB>round
#   review-pending.tsv   pr<TAB>round<TAB>sha<TAB>attempts<TAB>launched_epoch
#   watch.log            everything the watcher did
#
# Provider-overload rule (fleet.review-provider-overload — change transport,
# never the model): on a dead/empty verdict, retry pi once, then
# `edda dispatch --agent codex`; if that also yields no verdict, label the PR
# `review:unreviewed` and stop for that head. An unreviewed PR is an honest
# state; a cheap-model verdict is not.
#
# The watcher NEVER merges. Merge stays behind operator authorization
# (pr.merge-policy).
set -u

REPO=${EDDA_REPO:-fagemx/edda}
MODEL=${EDDA_REVIEW_MODEL:-openai-codex/gpt-5.6-sol}
POLL=${EDDA_REVIEW_POLL_SECONDS:-60}
SCRATCH=${EDDA_FLEET_SCRATCH:-$HOME/.edda/fleet}
STALE=${EDDA_REVIEW_STALE_SECONDS:-2700}   # 45 min: pi task limit is 30 min

SCRIPT_DIR=$(cd "$(dirname "$0")" && pwd)
REVIEW_PR="$SCRIPT_DIR/review-pr.sh"

ONCE=0
DRY=0
TAB=$(printf '\t')   # literal tab; "\t" inside quotes is backslash-t in POSIX sh
for a in "$@"; do
  case "$a" in
    --once) ONCE=1 ;;
    --dry-run) DRY=1 ;;
    -h|--help) echo "usage: pr-review-watch.sh [--once] [--dry-run]"; exit 0 ;;
    *) echo "pr-review-watch.sh: unexpected argument '$a'" >&2; exit 1 ;;
  esac
done

mkdir -p "$SCRATCH"
STATE="$SCRATCH/review-state.tsv"
PENDING="$SCRATCH/review-pending.tsv"
WATCHLOG="$SCRATCH/watch.log"
touch "$STATE" "$PENDING"

log() { echo "$(date -u '+%Y-%m-%dT%H:%M:%SZ') $*" >> "$WATCHLOG"; }

ensure_labels() {
  gh label create "review:lgtm"             --repo "$REPO" --color 0e8a16 --description "Automatic review verdict: LGTM (P0=0, P1=0)"            --force >/dev/null 2>&1 || true
  gh label create "review:changes-requested" --repo "$REPO" --color d93f0b --description "Automatic review verdict: Changes Requested"            --force >/dev/null 2>&1 || true
  gh label create "review:unreviewed"        --repo "$REPO" --color d4c5f9 --description "Review provider exhausted; verdict pending human/retry" --force >/dev/null 2>&1 || true
}

state_field() { # $1=pr  $2=field index (2=sha,3=round)
  awk -F'\t' -v pr="$1" -v f="$2" '$1==pr {print $f; found=1} END{if(!found) print ""}' "$STATE"
}

pending_has() { awk -F'\t' -v pr="$1" '$1==pr{found=1} END{exit !found}' "$PENDING" 2>/dev/null; }

pending_update() { # $1=pr $2=round $3=sha $4=attempts $5=launched
  awk -F'\t' -v pr="$1" '$1!=pr' "$PENDING" > "$PENDING.tmp"
  printf '%s\t%s\t%s\t%s\t%s\n' "$1" "$2" "$3" "$4" "$5" >> "$PENDING.tmp"
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

# Model observed + cost, read from the pi session files (never from what the
# brief asked for).
session_model_cost() { # $1=session id -> sets OBSERVED, COST
  files=$(find "$HOME/.pi/agent/sessions" -name "*_$1.jsonl" 2>/dev/null)
  if [ -n "$files" ]; then
    OBSERVED=$(cat $files 2>/dev/null | grep -o '"modelId":"[^"]*"' | tail -1 | sed 's/.*:"//;s/"$//')
    COST=$(cat $files 2>/dev/null | grep -o '"cost":{[^}]*}' | sed 's/.*"total"://' \
      | awk '{s+=$1} END{if (s=="") s=0; printf "%.2f", s}')
  else
    OBSERVED=""
    COST=""
  fi
  [ -n "${OBSERVED:-}" ] || OBSERVED="unknown"
  [ -n "${COST:-}" ] || COST="?"
}

pr_is_open() { # $1=pr
  [ "$(gh pr view "$1" --repo "$REPO" --json state --jq .state 2>/dev/null)" = "OPEN" ]
}

pr_labels() { # $1=pr
  gh pr view "$1" --repo "$REPO" --json labels --jq '[.labels[].name] | join(",")' 2>/dev/null
}

pr_head() { # $1=pr
  gh pr view "$1" --repo "$REPO" --json headRefOid --jq .headRefOid 2>/dev/null
}

# Post a verdict (or the honest unreviewed outcome) and update state.
post_verdict() { # $1=pr $2=round $3=sha $4=verdict file $5=transport note
  COMMENT="$SCRATCH/review-pr$1-r$2-comment.md"
  {
    echo "> **Round $2 review** — automatic watcher (\`scripts/pr-review-watch.sh\`): $5, read-only, detached worktree at \`$3\`. Model observed in the pi session file: \`$OBSERVED\`. Cost: \$$COST. Reviewed head SHA: \`$3\`."
    echo
    cat "$4"
  } > "$COMMENT"
  if [ "$DRY" = "1" ]; then
    echo "dry-run: would post $COMMENT to PR #$1 and set a review:* label"
    return 0
  fi
  gh pr comment "$1" --repo "$REPO" --body-file "$COMMENT" >/dev/null &&
    log "pr$1 r$2 posted verdict comment ($4)"
  if pr_is_open "$1" && [ "$(pr_head "$1")" = "$3" ]; then
    if grep -q '^LGTM' "$4"; then
      gh pr edit "$1" --repo "$REPO" --add-label "review:lgtm" >/dev/null 2>&1 &&
        log "pr$1 labeled review:lgtm"
      gh pr edit "$1" --repo "$REPO" --remove-label "review:changes-requested" >/dev/null 2>&1 || true
      gh pr edit "$1" --repo "$REPO" --remove-label "review:unreviewed" >/dev/null 2>&1 || true
    else
      gh pr edit "$1" --repo "$REPO" --add-label "review:changes-requested" >/dev/null 2>&1 &&
        log "pr$1 labeled review:changes-requested"
      gh pr edit "$1" --repo "$REPO" --remove-label "review:lgtm" >/dev/null 2>&1 || true
      gh pr edit "$1" --repo "$REPO" --remove-label "review:unreviewed" >/dev/null 2>&1 || true
    fi
  else
    log "pr$1 head moved or PR closed; verdict stays pinned to $3, label skipped"
  fi
  state_set "$1" "$3" "$2"
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

# ---- in-flight handling ------------------------------------------------------
settle_pending() {
  [ -s "$PENDING" ] || return 0
  now=$(date +%s)
  while IFS="$TAB" read -r pr round sha attempts launched < "$PENDING"; do
    [ -n "${pr:-}" ] || continue

    if ! pr_is_open "$pr"; then
      log "pr$pr closed/merged while review in flight; dropping pending entry"
      pending_drop "$pr"
      continue
    fi

    DONE="$SCRATCH/review-pr$pr-r$round.done"
    LOG="$SCRATCH/review-pr$pr-r$round.log"
    VERDICT="$SCRATCH/review-pr$pr-r$round-verdict.md"
    CODEXLOG="$SCRATCH/review-pr$pr-r$round-codex.log"

    is_done=0
    is_stale=0
    [ -f "$DONE" ] && is_done=1
    if [ "$is_done" = "0" ] && [ $((now - launched)) -gt "$STALE" ]; then
      is_stale=1
      log "pr$pr r$round review stale (${STALE}s, no .done); treating as dead"
    fi

    if [ "$is_done" = "1" ] || [ "$is_stale" = "1" ]; then
      if extract_verdict "$LOG" "$VERDICT" && verdict_ok "$VERDICT"; then
        session_model_cost "review-pr$pr"
        post_verdict "$pr" "$round" "$sha" "$VERDICT" \
          "pi -p --model $MODEL --thinking high --exclude-tools edit,write --session-id review-pr$pr"
        pending_drop "$pr"
      else
        case "$attempts" in
          0)
            log "pr$pr r$round dead/empty verdict (attempt 0) — retrying once with pi (same model, fleet.review-provider-overload)"
            if [ "$DRY" = "1" ]; then
              echo "dry-run: would retry PR #$pr r$round with pi"
              pending_drop "$pr"
            else
              "$REVIEW_PR" "$pr" "$round" "$sha" >> "$WATCHLOG" 2>&1 &&
                log "pr$pr r$round pi retry launched"
              pending_update "$pr" "$round" "$sha" 1 "$(date +%s)"
            fi
            ;;
          1)
            log "pr$pr r$round pi retry also dead (attempt 1) — switching transport: edda dispatch --agent codex (same subscribed model)"
            if [ "$DRY" = "1" ]; then
              echo "dry-run: would run edda dispatch --agent codex for PR #$pr r$round"
              pending_drop "$pr"
            else
              WT="$SCRATCH/wt-review-pr$pr"
              BRIEF="$SCRATCH/review-pr$pr-r$round-brief.md"
              rm -f "$CODEXLOG"
              if edda dispatch --agent codex --prompt-file "$BRIEF" --cwd "$WT" > "$CODEXLOG" 2>&1; then
                if extract_verdict "$CODEXLOG" "$VERDICT" && verdict_ok "$VERDICT"; then
                  OBSERVED="via edda dispatch --agent codex (pi session unavailable)"
                  COST="?"
                  post_verdict "$pr" "$round" "$sha" "$VERDICT" \
                    "edda dispatch --agent codex (transport fallback after pi overload)"
                  pending_drop "$pr"
                  continue
                fi
              fi
              mark_unreviewed "$pr" "$sha" "$round" "pi retry and codex dispatch both yielded no verdict"
              pending_drop "$pr"
            fi
            ;;
          *)
            mark_unreviewed "$pr" "$sha" "$round" "no verdict after pi retry and codex dispatch"
            pending_drop "$pr"
            ;;
        esac
      fi
    fi
  done
}

# ---- open-PR scan ------------------------------------------------------------
scan_open_prs() {
  rows=$(gh pr list --state open --repo "$REPO" --json number,headRefOid,isDraft,labels \
    --jq '.[] | select(.isDraft|not) | [.number, .headRefOid, ([.labels[].name] | join(","))] | @tsv' 2>/dev/null) || {
    log "gh pr list failed; skipping this cycle"
    return 0
  }
  printf '%s\n' "$rows" | while IFS="$TAB" read -r pr sha labels; do
    [ -n "${pr:-}" ] || continue
    pending_has "$pr" && continue

    reviewed=$(state_field "$pr" 2)
    round=$(state_field "$pr" 3)
    [ -n "$round" ] || round=0

    [ "$sha" = "$reviewed" ] && continue
    case ",$labels," in
      *,review:unreviewed,*)
        # unreviewed blocks only the head it was recorded for (per-head stop)
        if [ "$sha" = "$reviewed" ]; then continue; fi
        if [ "$DRY" != "1" ]; then
          gh pr edit "$pr" --repo "$REPO" --remove-label "review:unreviewed" >/dev/null 2>&1 || true
        fi
        ;;
    esac

    newround=$((round + 1))
    if [ "$DRY" = "1" ]; then
      prev=$(state_field "$pr" 2)
      [ -n "$prev" ] || prev=""
      echo "dry-run: would launch review for PR #$pr round $newround (head $sha${prev:+, prev $prev}): $REVIEW_PR $pr $newround $prev"
      log "dry-run: would launch review for pr$pr r$newround (head $sha)"
    else
      log "pr$pr head $sha differs from reviewed '${reviewed:-none}' — launching review r$newround"
      "$REVIEW_PR" "$pr" "$newround" "$reviewed" >> "$WATCHLOG" 2>&1 &&
        log "pr$pr r$newround launched (task edda-review-pr$pr-r$newround)"
      pending_update "$pr" "$newround" "$sha" 0 "$(date +%s)"
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
  settle_pending
  scan_open_prs
  [ "$ONCE" = "1" ] && break
  sleep "$POLL"
done
log "watcher cycle complete (once=$ONCE dry=$DRY)"
