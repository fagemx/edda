#!/bin/sh
# Single-PR read-only review dispatcher (GH-632).
#
# Launches the read-only reviewer for one PR, posts the SHA-pinned verdict as
# a PR comment, and applies the matching review label. Also usable standalone
# (manual re-review of one PR) — the watcher calls it the same way.
#
# Usage:
#   review-pr.sh <pr-number> [expected-head-sha]
#   review-pr.sh verdict-label          # stdin: comment body; stdout: label
#
# Hard rules (ratified decisions):
#   - reviewer is ALWAYS read-only: pi runs with --exclude-tools edit,write;
#     the codex fallback runs through `edda dispatch --agent codex` whose
#     prompt carries the same read-only constraint (改運輸不降模型).
#   - model is FIXED for the transition period (fleet.agent-model-split:
#     審查一律 gpt-5.6-sol). It is never downgraded to a cheaper model.
#   - overload ladder (fleet.review-provider-overload): pi same-model retry
#     (minimal-thinking probe, then full retry) → `edda dispatch --agent
#     codex` → if all transports fail, label `review:unreviewed` and stop.
#     未審查是誠實狀態；便宜模型的判決不是。
#   - this script NEVER merges (pr.merge-policy). No merge commands here.

set -eu

REVIEW_MODEL='openai-codex/gpt-5.6-sol'

usage() {
    printf 'usage: %s <pr-number> [expected-head-sha]\n' "$0" >&2
    printf '       %s verdict-label   (body on stdin, label on stdout)\n' "$0" >&2
    exit 2
}

log() {
    printf '%s review-pr: %s\n' "$(date -u +%Y-%m-%dT%H:%M:%SZ)" "$*"
}

# verdict-label — map verdict text (stdin) to a review label, or nothing.
# Rule: whichever verdict keyword appears LAST in the body wins; a review
# that mentions "LGTM" only inside quoted policy text still maps correctly.
verdict_label() {
    body=$(cat)
    lgtm_line=$(printf '%s' "$body" | grep -n 'LGTM' | tail -n 1 | cut -d: -f1 || true)
    changes_line=$(printf '%s' "$body" | grep -n 'Changes Requested' | tail -n 1 | cut -d: -f1 || true)
    if [ -n "$lgtm_line" ] && [ -n "$changes_line" ]; then
        if [ "$lgtm_line" -gt "$changes_line" ]; then
            printf 'review:lgtm'
        else
            printf 'review:changes-requested'
        fi
    elif [ -n "$lgtm_line" ]; then
        printf 'review:lgtm'
    elif [ -n "$changes_line" ]; then
        printf 'review:changes-requested'
    fi
}

# field <json> <jq-filter> — extract a field from captured PR JSON.
field() {
    printf '%s' "$1" | jq -r "$2"
}

# write_brief <file> <pr-number> <sha> <round> <title>
write_brief() {
    _file=$1 _number=$2 _sha=$3 _round=$4 _title=$5
    cat >"$_file" <<EOF
# Review brief — PR #${_number}「${_title}」（watcher 自動派審，Round ${_round}）

你是這個 PR 的審查者。**唯讀**：不改檔、不 push、不 merge、不刪分支或 worktree。
你的產出是**一則 PR 留言**（純文字，會被原樣貼上）。

## 引擎與身分
${REVIEW_MODEL}。一個 PR 一個審查者身分；本輪是 **Round ${_round}**。

## 框架：驗證清單（不是攻擊計畫）
逐條確認契約有沒有被滿足，指出**確實會出錯的地方**。每個發現都要能寫出
「什麼輸入／狀態 → 什麼錯誤輸出或崩潰」。寫不出來的就不是發現。

## 第一步：讀，不要急著跑
1. \`gh pr view ${_number}\` 與 \`gh pr diff ${_number}\` — 被審的面（head：${_sha}）。
2. 對應 issue 的 \`gh issue view <n>\` — **doneWhen 是驗收天花板**。超出它的期待寫成 FOLLOW-UP，不要當 blocking。
3. \`gh pr checks ${_number}\` — exact-head CI。**先 READ 這個**。
4. PR body 的 gate receipt（full SHA、跑過的閘、toolchain、lane、結果）。**先 READ 它**。
5. **增量審查**：若 PR 上已有先前 Round 的審查留言，讀它們（\`gh pr view ${_number} --comments\`）；
   只審新 head 的 diff 與對先前發現的回應，不重審已 LGTM 的面。

## 逐條檢查
1. doneWhen 逐條對照；2. FAIL-first 證據（red→green，不是宣稱）；3. 接線四問（誰寫／誰讀／
失敗訊號／能力到哪層）——有新面卻沒有讀者或失敗無訊號 → P0/P1；4. FORBIDDEN 邊界；
5. 與現行 origin/main 的語意整合；6. 安全／資料遺失。

## 範圍凍結
IN SCOPE：改動的行為與路徑、直接呼叫者、issue 驗收條件、本改動引入或暴露的安全／資料遺失回歸、
與現行 base 的整合。其餘有證據的問題寫成 FOLLOW-UP ISSUE，不用來擋這個 PR。

## 產出格式（會被原樣貼成 PR 留言）
- 標題行：\`## Code Review: Round ${_round}\`
- 釘住的 full head SHA：${_sha}
- \`model_observed:\` 你實際觀察到的模型名
- \`cost:\` 大略 token／時間
- blocking P0／P1 清單（file:line + 什麼情況會壞）；RAN vs READ 分列
- 結論行：\`Changes Requested\` 或 \`LGTM\`（P0=0 且 P1=0 才可以寫 LGTM；不確定標 \`[判斷]\`）

**不要 merge。** 合併是操作者的權限。
EOF
}

review_pr() {
    number=$1
    expected=${2:-}

    command -v gh >/dev/null || { log 'gh not found' >&2; exit 1; }
    command -v jq >/dev/null || { log 'jq not found' >&2; exit 1; }

    pr_json=$(gh pr view "$number" --json number,headRefOid,isDraft,title) || {
        log "cannot read PR #$number"; exit 1
    }
    if [ "$(field "$pr_json" '.isDraft // false')" = "true" ]; then
        log "PR #$number is a draft — not reviewing"
        exit 0
    fi
    sha=$(field "$pr_json" '.headRefOid // ""')
    if [ -z "$sha" ]; then
        log "PR #$number has no head SHA — not reviewing"
        exit 1
    fi
    if [ -n "$expected" ] && [ "$expected" != "$sha" ]; then
        log "PR #$number head moved ($expected -> $sha) — aborting stale dispatch" >&2
        exit 1
    fi
    title=$(field "$pr_json" '.title')

    # Round = 1 + prior review comments. Comment bodies begin with the
    # watcher marker, so match anywhere, not startswith.
    round=$(gh api "repos/{owner}/{repo}/issues/$number/comments" \
        --jq '[.[].body | select(contains("## Code Review: Round"))] | length') || round=0
    round=$((round + 1))

    tmp=$(mktemp -d)
    trap 'rm -rf "$tmp"' 0 HUP INT TERM
    brief=$tmp/brief.md
    write_brief "$brief" "$number" "$sha" "$round" "$title"

    prompt="讀 $brief 並完全照它審 PR #$number。你是唯讀審查者：不可修改檔案、不可 push、不可 merge。產出是一則會貼到 PR #$number 的留言。"

    verdict=''
    transport=''
    cost_line=''

    # Transport 1: pi, same model, full thinking.
    if pi -p --model "$REVIEW_MODEL" --thinking high \
        --exclude-tools edit,write --session-id "review-pr$number" \
        "$prompt" >"$tmp/verdict.md" 2>"$tmp/pi1.log"; then
        transport='pi (--thinking high)'
    else
        # Overload path: probe with minimal thinking, then retry same model.
        if pi -p --model "$REVIEW_MODEL" --thinking minimal \
            --exclude-tools edit,write --session-id "review-pr$number-probe" \
            '連線探測：只回 OK' >"$tmp/probe.md" 2>"$tmp/pi-probe.log"; then
            if pi -p --model "$REVIEW_MODEL" --thinking high \
                --exclude-tools edit,write --session-id "review-pr$number" \
                "$prompt" >"$tmp/verdict.md" 2>"$tmp/pi2.log"; then
                transport='pi (minimal-thinking probe, then --thinking high)'
            fi
        fi
    fi

    # Transport 2: codex via edda dispatch — same model family, different transport.
    if [ -z "$transport" ]; then
        log "pi transport unavailable for PR #$number; falling back to edda dispatch --agent codex"
        if dispatch_json=$(edda dispatch --agent codex \
            --session-id "review-pr$number" \
            --prompt-file "$brief" --json 2>"$tmp/dispatch.log"); then
            result=$(printf '%s' "$dispatch_json" | jq -r '.result_text // ""')
            cost_usd=$(printf '%s' "$dispatch_json" | jq -r '.cost_usd // empty')
            if [ -n "$result" ] && [ "$result" != "null" ]; then
                printf '%s\n' "$result" >"$tmp/verdict.md"
                transport='edda dispatch --agent codex'
                if [ -n "$cost_usd" ]; then
                    cost_line="\$${cost_usd} (edda dispatch)"
                fi
            fi
        fi
    fi

    # All transports failed: label honestly and stop. Never downgrade the model.
    if [ -z "$transport" ]; then
        log "all transports failed for PR #$number — labeling review:unreviewed and stopping" >&2
        gh pr edit "$number" --add-label 'review:unreviewed' >/dev/null 2>&1 || true
        exit 1
    fi

    body=$(cat "$tmp/verdict.md")
    observed=$(printf '%s\n' "$body" | sed -n 's/^[#* -]*model_observed: */model_observed: /p' | tail -n 1)
    reported_cost=$(printf '%s\n' "$body" | sed -n 's/^[#* -]*cost: */cost: /p' | tail -n 1)
    [ -n "$cost_line" ] || cost_line=${reported_cost:-'未回報（見內文）'}

    {
        printf '<!-- pr-review-watcher: posted by scripts/pr-review-watch.sh; the watcher never merges -->\n\n'
        printf '## Code Review: Round %s\n\n' "$round"
        printf -- '- Head SHA (pinned): `%s`\n' "$sha"
        printf -- '- Model (requested): `%s`\n' "$REVIEW_MODEL"
        printf -- '- %s\n' "${observed:-model_observed: 審查者未回報（requested 見上）}"
        printf -- '- Transport: %s\n' "$transport"
        printf -- '- Cost: %s\n' "$cost_line"
        printf '\n---\n\n%s\n' "$body"
    } >"$tmp/body.md"

    gh pr comment "$number" --body-file "$tmp/body.md" >/dev/null
    log "posted Round $round verdict for PR #$number ($transport)"

    label=$(verdict_label <"$tmp/verdict.md")
    if [ -n "$label" ]; then
        gh pr edit "$number" --add-label "$label" >/dev/null
        log "PR #$number labeled $label"
    fi
}

case ${1:-} in
    verdict-label)
        [ $# -eq 1 ] || usage
        verdict_label
        ;;
    ''|--help|-h)
        usage
        ;;
    *)
        [ $# -le 2 ] || usage
        review_pr "$@"
        ;;
esac
