#!/bin/sh
set -eu

root=$(CDPATH= cd -- "$(dirname "$0")/.." && pwd)
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' 0 HUP INT TERM

canonical="$root/crates/edda-cli/src/skills/coord-orchestrate.md"
projection="$root/.claude/skills/coord-orchestrate/SKILL.md"
pipeline="$root/.claude/skills/issue-pipeline/SKILL.md"
action="$root/.claude/skills/issue-action/SKILL.md"
self_check="$root/.claude/skills/pr-review-loop/SKILL.md"

fail() {
    printf 'delivery guidance fixture failed: %s\n' "$*" >&2
    exit 1
}

require_text() {
    file=$1
    text=$2
    grep -F -- "$text" "$file" >/dev/null || fail "$file lacks: $text"
}

reject_text() {
    file=$1
    text=$2
    if grep -F -- "$text" "$file" >/dev/null; then
        fail "$file contains forbidden active text: $text"
    fi
}

# Quoted historical counterexamples are evidence, not active procedure.
active_copy() {
    input=$1
    output=$2
    awk '!/^[[:space:]]*>/' "$input" >"$output"
}

audit_coord() {
    input=$1
    active="$tmp/active-coord"
    active_copy "$input" "$active"

    require_text "$active" 'delivery-flow/1'
    require_text "$active" 'Small single-owner change'
    require_text "$active" 'delivery.rail-owner=manual:$CONTROLLER_SESSION'
    require_text "$active" 'not authenticated'
    require_text "$active" 'does not update title, assignee, brief, scope or'
    require_text "$active" 'SAME ID'
    require_text "$active" 'Changed owner, brief, scope or dependency graph is replacement'
    require_text "$active" '| Pi | task-linked `--prompt-file` plus explicit `--cwd`'
    require_text "$active" '| Claude | task-linked `--prompt-file` plus explicit `--cwd`'
    require_text "$active" '| Codex | task-linked `--prompt-file` plus explicit `--cwd`'
    require_text "$active" '| ACP target | separately created task with matching `--agent acp:<target>`; `--task-id`'
    require_text "$active" 'It does not prove the brief was readable.'
    require_text "$active" 'product-verifiable authority'
    require_text "$active" 'one combined self-check activity'
    require_text "$active" '**Behavior lens:**'
    require_text "$active" '**Counterexample lens:**'
    require_text "$active" 'exact-head CI'
    require_text "$active" 'Never impose an all-agent phase barrier.'

    reject_text "$active" 'delivery.rail-owner.$PLAN_KEY'
    if grep -E '^[[:space:]]*(Wait for (all|ALL)|gh pr merge|cargo (test|clippy|check) --workspace)' "$active" >/dev/null; then
        fail "$input contains an active phase barrier, direct merge, or full-workspace command"
    fi
}

audit_routes() {
    for file in "$pipeline" "$action" "$self_check"; do
        lines=$(wc -l <"$file" | tr -d ' ')
        [ "$lines" -le 70 ] || fail "$file is no longer a thin route ($lines lines)"
        require_text "$file" 'delivery-flow/1'
        reject_text "$file" '## Pipeline Phases'
        reject_text "$file" 'ACTION: REVIEW'
        if grep -E '^[[:space:]]*(Wait for (all|ALL)|gh pr merge|cargo (test|clippy|check) --workspace)' "$file" >/dev/null; then
            fail "$file revived an active duplicate delivery loop"
        fi
    done

    require_text "$pipeline" '`--skip-plan` means the caller says usable acceptance already exists.'
    require_text "$pipeline" '`--no-merge` stops this invocation before merge.'
    require_text "$pipeline" 'never grants merge'
    require_text "$pipeline" 'all-issue phase barrier'
    require_text "$action" 'read `edda task show <id>`'
    require_text "$action" 'If acceptance is materially unclear'
    require_text "$action" 'record its behavior and counterexample lenses together'
    require_text "$self_check" 'It is not an'
    require_text "$self_check" '**Behavior lens:**'
    require_text "$self_check" '**Counterexample lens:**'
    require_text "$self_check" 'no direct merge command'
}

expect_coord_failure() {
    name=$1
    file=$2
    if (audit_coord "$file") >/dev/null 2>&1; then
        fail "mutated coord case passed: $name"
    fi
}

expect_route_failure() {
    name=$1
    if (audit_routes) >/dev/null 2>&1; then
        fail "mutated route case passed: $name"
    fi
}

cmp -s "$canonical" "$projection" || fail 'tracked coord projection differs from authored source'
audit_coord "$canonical"
audit_routes

# A quoted old mistake remains inspectable without becoming an active command.
cp "$canonical" "$tmp/quoted.md"
printf '%s\n' '> Historical bad example: Wait for all agents, then gh pr merge 7.' >>"$tmp/quoted.md"
audit_coord "$tmp/quoted.md"

# Semantic mutations must fail independently; this is not a keyword-only happy path.
sed 's/delivery\.rail-owner=manual:/delivery.rail-owner.$PLAN_KEY=manual:/' \
    "$canonical" >"$tmp/per-plan-owner.md"
expect_coord_failure 'per-plan rail owner' "$tmp/per-plan-owner.md"

cp "$canonical" "$tmp/phase-barrier.md"
printf '%s\n' 'Wait for ALL agents before any bundle can review.' >>"$tmp/phase-barrier.md"
expect_coord_failure 'all-agent phase barrier' "$tmp/phase-barrier.md"

sed 's/controller starts the SAME ID/controller creates a NEW ID/' \
    "$canonical" >"$tmp/new-id-retry.md"
expect_coord_failure 'retry changed to replacement' "$tmp/new-id-retry.md"

sed 's/It does not prove the brief was readable\./It proves the brief was readable./' \
    "$canonical" >"$tmp/acp-brief.md"
expect_coord_failure 'ACP preflight overclaim' "$tmp/acp-brief.md"

sed 's/not authenticated/authenticated/' "$canonical" >"$tmp/runtime-enforcement.md"
expect_coord_failure 'caller discipline claimed as runtime authority' "$tmp/runtime-enforcement.md"

cp "$canonical" "$tmp/direct-merge.md"
printf '%s\n' 'gh pr merge 7 --squash' >>"$tmp/direct-merge.md"
expect_coord_failure 'direct forge merge' "$tmp/direct-merge.md"

cp "$canonical" "$tmp/full-workspace.md"
printf '%s\n' 'cargo test --workspace' >>"$tmp/full-workspace.md"
expect_coord_failure 'full workspace on freeze' "$tmp/full-workspace.md"

# Mutate each legacy route through the same global variables used by audit_routes.
original_pipeline=$pipeline
cp "$original_pipeline" "$tmp/pipeline-bad.md"
printf '%s\n' '## Pipeline Phases' 'Wait for all issues before review.' >>"$tmp/pipeline-bad.md"
pipeline="$tmp/pipeline-bad.md"
expect_route_failure 'pipeline phase loop'
pipeline=$original_pipeline

original_action=$action
sed 's/If acceptance is materially unclear/If acceptance may be skipped/' \
    "$original_action" >"$tmp/action-bad.md"
action="$tmp/action-bad.md"
expect_route_failure 'issue action acceptance bypass'
action=$original_action

original_self_check=$self_check
cp "$original_self_check" "$tmp/self-check-bad.md"
printf '%s\n' 'gh pr merge 8 --squash' >>"$tmp/self-check-bad.md"
self_check="$tmp/self-check-bad.md"
expect_route_failure 'author self-check merge loop'
self_check=$original_self_check

# Source anchors keep the prose claims tied to the current substrate.
require_text "$root/crates/edda-ledger/src/task_actions.rs" 'Ready = normal start; Failed = retry.'
require_text "$root/crates/edda-ledger/src/task_actions.rs" 'if let Some(existing) = tasks::find_by_idempotency_key'
require_text "$root/crates/edda-cli/src/cmd_dispatch.rs" '--task-id is only valid with an ACP agent'
require_text "$root/crates/edda-cli/src/cmd_dispatch_acp.rs" 'must be running before an agent turn'
require_text "$root/crates/edda-cli/src/cmd_dispatch_acp.rs" 'task agent_kind must match selected ACP target'
require_text "$root/crates/edda-cli/src/cmd_dispatch_acp.rs" 'use a concrete permission root'
require_text "$root/crates/edda-cli/src/cmd_dispatch_acp.rs" '[brief truncated]'
require_text "$root/crates/edda-cli/src/cmd_reconcile/plan.rs" 'TaskStatus::Ready if slots > 0'
require_text "$root/crates/edda-cli/src/cmd_reconcile/plan.rs" 'TaskStatus::Failed'
if grep -F 'plan_id' "$root/crates/edda-cli/src/cmd_reconcile/plan.rs" >/dev/null; then
    fail 'reconcile planning unexpectedly filters by plan_id; revisit rail-owner guidance'
fi

# Caller-mode model: repository-wide conflicts and unknown reconcile evidence refuse.
manual_allowed() {
    owner=$1
    scheduler=$2
    process=$3
    attempts=$4
    [ "$owner" = manual ] && [ "$scheduler" = absent ] && \
        [ "$process" = absent ] && [ "$attempts" = settled ]
}
manual_allowed manual absent absent settled || fail 'clean manual rail refused'
if manual_allowed conflict absent absent settled; then fail 'cross-plan split owner accepted'; fi
if manual_allowed manual unknown absent settled; then fail 'unknown scheduler accepted'; fi
if manual_allowed manual absent active settled; then fail 'one-off reconcile accepted'; fi
if manual_allowed manual absent absent live; then fail 'live reconcile attempt accepted'; fi
if manual_allowed reconcile absent absent settled; then fail 'reconcile rail accepted manual start'; fi

# Optional real CLI substrate. Set EDDA_BIN to the freshly built candidate.
if [ -n "${EDDA_BIN:-}" ]; then
    "$EDDA_BIN" --version >/dev/null 2>&1 || fail "EDDA_BIN is not runnable: $EDDA_BIN"
    repo="$tmp/repo"
    export EDDA_STORE_ROOT="$tmp/store"
    mkdir -p "$repo/work" "$repo/briefs" "$EDDA_STORE_ROOT"
    printf 'A acceptance\n' >"$repo/briefs/a.md"
    printf 'ACP full acceptance\n' >"$repo/briefs/acp.md"
    printf 'evidence\n' >"$repo/evidence.txt"
    git -C "$repo" init -q

    run_edda() {
        (cd "$repo" && "$EDDA_BIN" "$@")
    }

    run_edda init --no-hooks >/dev/null
    [ ! -d "$repo/.claude/skills" ] || fail 'no-host init projected Claude skills'
    [ ! -d "$repo/.agents/skills" ] || fail 'no-host init projected Codex skills'

    run_edda decide 'delivery.rail-owner=manual:fixture-controller' \
        --session fixture-controller --reason 'isolated fixture: no scheduler/process/attempt' >/dev/null
    run_edda task new 'A delivery' --assignee worker-a \
        --plan demo/r0001/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa \
        --brief briefs/a.md --path work \
        --key demo/r0001/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa/A >/dev/null
    run_edda task new 'MUTATED TWIN' --assignee wrong-owner \
        --plan demo/r0001/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa \
        --brief missing.md --path work \
        --key demo/r0001/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa/A >/dev/null
    run_edda task show 1 --json >"$tmp/task-a.json"
    require_text "$tmp/task-a.json" '"title": "A delivery"'
    require_text "$tmp/task-a.json" '"assignee": "worker-a"'
    require_text "$tmp/task-a.json" '"brief_ref": "briefs/a.md"'
    reject_text "$tmp/task-a.json" 'MUTATED TWIN'
    reject_text "$tmp/task-a.json" 'wrong-owner'

    run_edda task new 'B independent' --assignee worker-b \
        --plan demo/r0001/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa \
        --brief briefs/a.md --path work >/dev/null
    run_edda task new 'C after A' --assignee worker-c --after 1 \
        --plan demo/r0001/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa \
        --brief briefs/a.md --path work >/dev/null
    run_edda decide \
        'delivery.active.demo=demo/r0001/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa' \
        --session fixture-controller --reason 'A=#1 B=#2 C=#3; rail-owner=manual' >/dev/null

    run_edda ask delivery.rail-owner --json >"$tmp/rail.json"
    run_edda ask delivery.active.demo --json >"$tmp/map.json"
    run_edda task list --json >"$tmp/tasks-before.json"
    require_text "$tmp/rail.json" '"value": "manual:fixture-controller"'
    require_text "$tmp/map.json" '"value": "demo/r0001/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"'
    require_text "$tmp/tasks-before.json" '"plan_id": "demo/r0001/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"'

    run_edda task start 1 >"$tmp/start-one.txt"
    require_text "$tmp/start-one.txt" 'attempt 1'
    if run_edda task start 1 >/dev/null 2>&1; then fail 'second controller start succeeded'; fi
    run_edda task fail 1 --reason 'observed stopped fixture' >/dev/null
    run_edda task show 1 --json >"$tmp/failed-a.json"
    run_edda task show 2 --json >"$tmp/ready-b.json"
    run_edda task show 3 --json >"$tmp/blocked-c.json"
    require_text "$tmp/failed-a.json" '"status": "failed"'
    require_text "$tmp/ready-b.json" '"status": "ready"'
    require_text "$tmp/blocked-c.json" '"status": "blocked"'

    run_edda task start 1 >"$tmp/start-two.txt"
    require_text "$tmp/start-two.txt" 'attempt 2'
    run_edda task done 1 --receipt 'A candidate verified' --evidence evidence.txt >/dev/null
    run_edda task show 3 --json >"$tmp/ready-c.json"
    require_text "$tmp/ready-c.json" '"status": "ready"'
    if run_edda task start 1 >/dev/null 2>&1; then fail 'done task restarted'; fi
    run_edda task done 1 --receipt 'receipt metadata corrected' --evidence evidence.txt >/dev/null
    run_edda task show 1 --json >"$tmp/corrected-a.json"
    require_text "$tmp/corrected-a.json" '"attempts": 2'
    require_text "$tmp/corrected-a.json" '"receipt": "receipt metadata corrected"'

    run_edda task new 'ACP task' --assignee acp-worker --agent acp:grok \
        --plan demo/r0001/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa \
        --brief briefs/acp.md --path work \
        --key demo/r0001/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa/ACP >/dev/null
    run_edda task show 4 --json >"$tmp/acp.json"
    require_text "$tmp/acp.json" '"agent_kind": "acp:grok"'
    require_text "$tmp/acp.json" '"brief_ref": "briefs/acp.md"'
    require_text "$tmp/acp.json" '"scope_paths"'
    if run_edda dispatch --agent acp:grok --task-id 4 --cwd "$repo" --json \
        >"$tmp/acp-ready.out" 2>"$tmp/acp-ready.err"; then
        fail 'ACP preflight accepted a task that was not Running'
    fi
    require_text "$tmp/acp-ready.err" 'must be running before an agent turn'

    run_edda task new 'Legacy lookalike' --assignee acp-worker \
        --plan demo/r0001/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa \
        --brief briefs/acp.md --path work >/dev/null
    if run_edda dispatch --agent acp:grok --task-id 5 --cwd "$repo" --json \
        >"$tmp/acp-legacy.out" 2>"$tmp/acp-legacy.err"; then
        fail 'ACP preflight reused a legacy task ID'
    fi
    require_text "$tmp/acp-legacy.err" 'task agent_kind must match selected ACP target'

    printf 'delivery guidance real CLI substrate passed\n'
else
    printf 'delivery guidance real CLI substrate not run (set EDDA_BIN to a fresh candidate)\n'
fi

printf 'delivery guidance fixtures passed\n'
