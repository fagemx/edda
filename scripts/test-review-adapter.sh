#!/bin/sh
# Offline GH-652 product-adapter checks. Runs only the selected edda-review
# child, so legacy dispatch fixtures remain independent regression coverage.
set -eu

root=$(CDPATH= cd -- "$(dirname "$0")/.." && pwd)
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' 0 HUP INT TERM
mkdir -p "$tmp/bin" "$tmp/scratch"
sha=aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa
export EDDA_FLEET_ROOT="$root" EDDA_FLEET_SCRATCH="$tmp/scratch" EDDA_REPO=fixture/repo
export EDDA_REVIEW_PRODUCT_ADAPTER=1 PATH="$tmp/bin:$PATH"
# Round reservation (scripts/review-round.sh) is machine-local shared state:
# the launch fixtures below must never write the real ~/.edda/review-coordination.
export EDDA_REVIEW_COORD_DIR="$tmp/coord"

cat >"$tmp/bin/uname" <<'EOF'
#!/bin/sh
echo "${ADAPTER_PLATFORM:-Linux}"
EOF
cat >"$tmp/bin/gh" <<EOF
#!/bin/sh
case "\$*" in
  *api*comments*) echo '[]' ;;
  *headRefOid*) echo '$sha' ;;
  *headRefName*|*baseRefName*) echo main ;;
  *baseRefOid*) echo bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb ;;
  *title*) echo fixture ;;
  *body*|*closingIssuesReferences*|*--name-only*) : ;;
esac
EOF
cat >"$tmp/bin/edda" <<EOF
#!/bin/sh
if [ "\$1 \$2" = 'review --help' ]; then echo '--pr N --agent AGENT --model MODEL --json --resume'; exit 0; fi
printf '%s\n' "\$*" >> "\$EDDA_FLEET_SCRATCH/argv"
[ -z "\${ADAPTER_SLEEP:-}" ] || sleep "\$ADAPTER_SLEEP"
case "\${ADAPTER_CASE:-ok}" in
  malformed) printf '{not json\n'; exit 2 ;;
  changed) proof=failed; policy=hard; head='$sha' ;;
  policy) proof=unchanged; policy=none; head='$sha' ;;
  wrong-head) proof=unchanged; policy=hard; head=cccccccccccccccccccccccccccccccccccccccc ;;
  unqualified) proof=unchanged; policy=hard; head='$sha'; verdict=lgtm; qualified=false; disqualifiers='["gates-red","escalation-pending"]'; exit_code=3 ;;
  detailed) proof=unchanged; policy=hard; head='$sha'; verdict=changes-requested; qualified=true; disqualifiers='[]'; exit_code=1 ;;
  removal-failed) proof=unchanged; policy=hard; head='$sha'; verdict=changes-requested; qualified=true; disqualifiers='[]'; exit_code=1; notes='worktree removal failed: fixture' ;;
  *) proof=unchanged; policy=hard; head='$sha'; verdict=changes-requested; qualified=true; disqualifiers='[]'; exit_code=1 ;;
esac
printf '{"event_id":"evt_fixture_review","subject":{"head_sha":"%s","subject_seen":"%s","worktree_check":"%s"},"reviewer":{"tool_policy":"%s","model_requested":"fixture-model","model_observed":"fixture-observed","session_id":"fixture-session"},"verdict":"%s","qualified":%s,"disqualifiers":%s,"findings":[{"severity":"P1","file":"scripts/review-pr.sh","line":261,"claim":"fixture claim","evidence":"fixture:evidence","rule":"C1","status":"open"}],"checklist":[{"item":"adapter fixture","result":"ran","measure":"fixture-measure"}],"escalations":["fixture escalation"],"cost":{"usd":0.25},"notes":"%s"}\n' "\$head" "\$head" "\$proof" "\$policy" "\${verdict:-changes-requested}" "\${qualified:-true}" "\${disqualifiers:-[]}" "\${notes:-}"
exit "\${exit_code:-1}"
EOF
chmod +x "$tmp/bin/uname" "$tmp/bin/gh" "$tmp/bin/edda"

run_case() { # name expected-check round
  name=$1 expected=$2 round=$3
  rm -f "$EDDA_FLEET_SCRATCH/review-pr9999-r$round.log" "$EDDA_FLEET_SCRATCH/review-pr9999-r$round.done" "$EDDA_FLEET_SCRATCH/argv"
  ADAPTER_CASE=$name timeout 20 "$root/scripts/review-pr.sh" 9999 "$round" --dry-run >/dev/null
  runner="$EDDA_FLEET_SCRATCH/review-pr9999-r$round-run.sh"
  ADAPTER_CASE=$name timeout 20 "$runner" || true
  done="$EDDA_FLEET_SCRATCH/review-pr9999-r$round.done"
  [ -f "$done" ] || { echo "$name: no terminal receipt" >&2; return 1; }
  grep -q "^WORKTREE_CHECK=$expected" "$done" || { cat "$done" >&2; echo "$name: unexpected worktree receipt" >&2; return 1; }
}

# The Verdict line `scripts/review-pr.sh verdict-label` reads from one round's
# envelope — the label the watcher would apply.
label_of() { # $1=lane log
  sed -n '/^<<<VERDICT$/,/^VERDICT>>>$/p' "$1" | sh "$root/scripts/review-pr.sh" verdict-label
}
# REVIEW.md §7 order: the Verdict line is the last line of the envelope, so a
# first-match reader can never take a finding's prose for the verdict (#998).
verdict_is_last() { # $1=lane log
  [ "$(sed -n '/^### Verdict$/,/^VERDICT>>>$/p' "$1" | wc -l | tr -d ' ')" = 3 ] \
    || { cat "$1" >&2; echo "$2: the Verdict line is not the last line of the envelope" >&2; exit 1; }
}

run_case ok unchanged 1
done="$EDDA_FLEET_SCRATCH/review-pr9999-r1.done"
log="$EDDA_FLEET_SCRATCH/review-pr9999-r1.log"
grep -q '^TRANSPORT=edda-review$' "$done"
grep -q '^POLICY_RECEIPT=product-json:hard$' "$done"
grep -q '^Changes Requested, P0=0, P1=1$' "$log"
if grep -q '^TOOL_FLAGS=' "$done"; then echo 'ok: legacy policy string was fabricated' >&2; exit 1; fi
grep -q 'nohup "\$RUNNER"' "$root/scripts/review-pr.sh"
# The product receipt is the same atomic terminal object the legacy lane
# publishes (#998): scripts/review-round.sh reserve and the watcher's terminal
# gate read FINAL_EXIT / WORKTREE_CLEANUP / TASK_CLEANUP / TERMINAL_RECEIPT,
# and a receipt without them blocks every later round of the PR.
for line in 'FINAL_EXIT=1' 'WORKTREE_CLEANUP=removed' 'TASK_CLEANUP=not-applicable' 'TERMINAL_RECEIPT=complete'; do
  grep -qx "$line" "$done" || { cat "$done" >&2; echo "ok: product receipt lacks $line" >&2; exit 1; }
done
# The envelope names the engine the way REVIEW.md §7 does, so the watcher's
# rules.md R22 executor can tell an authoritative round from a SHADOW one.
grep -qx -- '- model_observed: fixture-observed' "$log" || { cat "$log" >&2; echo 'ok: envelope carries no model_observed header' >&2; exit 1; }
grep -qx -- '- escalations: fixture escalation' "$log" || { cat "$log" >&2; echo 'ok: envelope carries no escalations header' >&2; exit 1; }
verdict_is_last "$log" ok
[ "$(label_of "$log")" = review:changes-requested ] || { echo 'ok: verdict-label did not read Changes Requested' >&2; exit 1; }

for case_name in changed policy wrong-head malformed; do run_case "$case_name" 'failed;' 1; done
ADAPTER_CASE=ok run_case ok unchanged 2
grep -q -- '--resume' "$EDDA_FLEET_SCRATCH/argv"
grep -q -- '--agent claude --model claude-opus-5 --json --resume' "$EDDA_FLEET_SCRATCH/argv"

# An unqualified LGTM has product exit 3 — provisional under REVIEW.md §6.4,
# never `LGTM (P0=0, P1=0)`, never the merge gate (§8). The adapter publishes
# the whole payload (findings, checklist, escalations) under a Verdict line
# that says so without the LGTM token, so verdict-label cannot resolve it to
# review:lgtm and the watcher posts it instead of discarding a $3 review (#998).
run_case unqualified unchanged 3
done="$EDDA_FLEET_SCRATCH/review-pr9999-r3.done"
log="$EDDA_FLEET_SCRATCH/review-pr9999-r3.log"
grep -q '^DISPATCH_EXIT=3$' "$done"
grep -q '^FINAL_EXIT=3$' "$done" || { cat "$done" >&2; echo 'unqualified: FINAL_EXIT does not carry the product exit' >&2; exit 1; }
grep -q '^QUALIFIED=false$' "$done"
grep -q '^DISQUALIFIERS=gates-red,escalation-pending$' "$done"
grep -q '^<<<VERDICT$' "$log" || { cat "$log" >&2; echo 'unqualified: exit 3 discarded the review payload (no verdict envelope)' >&2; exit 1; }
grep -qx 'Provisional — unqualified (disqualifiers: gates-red, escalation-pending), P0=0, P1=1 — not a merge-gate verdict' "$log" \
  || { cat "$log" >&2; echo 'unqualified: the Verdict line does not state the provisional outcome' >&2; exit 1; }
grep -q '"claim":"fixture claim"' "$log" || { echo 'unqualified: findings were not written' >&2; exit 1; }
grep -q '"item":"adapter fixture"' "$log" || { echo 'unqualified: checklist was not written' >&2; exit 1; }
grep -q 'fixture escalation' "$log" || { echo 'unqualified: escalations were not written' >&2; exit 1; }
if grep -qE '^(LGTM|Changes Requested)' "$log"; then echo 'unqualified: adapter emitted a merge-gate verdict line for an unqualified LGTM' >&2; exit 1; fi
verdict_is_last "$log" unqualified
vl=$(label_of "$log")
[ -z "$vl" ] || { echo "unqualified: verdict-label resolved the provisional round to '$vl'" >&2; exit 1; }

# The generated child retains the complete structured product material inside
# the verdict envelope that the watcher copies into its PR comment carrier.
run_case detailed unchanged 4
done="$EDDA_FLEET_SCRATCH/review-pr9999-r4.done"
log="$EDDA_FLEET_SCRATCH/review-pr9999-r4.log"
grep -q '^Event identity: evt_fixture_review$' "$log"
grep -q '^Qualification: true$' "$log"
grep -q '"file":"scripts/review-pr.sh"' "$log"
grep -q '"line":261' "$log"
grep -q '"evidence":"fixture:evidence"' "$log"
grep -q '"item":"adapter fixture"' "$log"
grep -q '^### Escalations$' "$log"
grep -q 'fixture escalation' "$log"
[ -s "$log.json" ] || { echo 'detailed: product JSON was discarded before a reviewer could inspect it' >&2; exit 1; }

# WORKTREE_CLEANUP relays the product's own report: `edda review` removes the
# internal worktree it added and records a failure only in `notes`.
run_case removal-failed unchanged 7
done="$EDDA_FLEET_SCRATCH/review-pr9999-r7.done"
grep -q '^WORKTREE_CLEANUP=failed' "$done" || { cat "$done" >&2; echo 'removal-failed: a product-reported removal failure was relayed as removed' >&2; exit 1; }

# First use starts without the state directory. The selected product path owns
# creation before it writes its lane/runner artifacts.
rm -rf "$EDDA_FLEET_SCRATCH"
ADAPTER_CASE=ok timeout 20 "$root/scripts/review-pr.sh" 9999 5 --dry-run >/dev/null
[ -d "$EDDA_FLEET_SCRATCH" ] || { echo 'fresh scratch: product adapter did not create its state directory' >&2; exit 1; }
[ -s "$EDDA_FLEET_SCRATCH/review-pr9999-r5-run.sh" ] || { echo 'fresh scratch: product runner was not generated' >&2; exit 1; }

# Not a dry run: the product path reserves the shared round exactly like the
# brief path and prints the same receipts — without `review_round=` the
# watcher logs "launcher returned no shared round receipt; refusing to guess
# artifact paths" and never tracks the round (#998). Round 2 is admitted only
# through round 1's clean terminal receipt, so a product receipt short of one
# would block every later round of the PR.
wait_done() { # $1=.done path
  for _ in $(seq 1 30); do [ -f "$1" ] && return 0; sleep 1; done
  echo "launch: no terminal receipt at $1" >&2; exit 1
}
launch() { # $1=case $2=round -> stdout of the launcher (fails loudly)
  if ! ADAPTER_CASE=$1 ADAPTER_SLEEP=2 timeout 30 "$root/scripts/review-pr.sh" 9999 "$2"; then
    echo "launch: round $2 ($1) failed to launch" >&2; exit 1
  fi
}
rm -f "$EDDA_FLEET_SCRATCH/argv"
out=$(launch unqualified 1)
for key in log done session review_round; do
  printf '%s\n' "$out" | grep -q "^$key=" || { printf '%s\n' "$out" >&2; echo "launch: product path printed no $key= receipt" >&2; exit 1; }
done
printf '%s\n' "$out" | grep -qx 'review_round=1' || { printf '%s\n' "$out" >&2; echo 'launch: round 1 was not reserved as round 1' >&2; exit 1; }
[ -f "$tmp/coord/fixture/repo/pr9999/active" ] || { echo 'launch: the product path reserved no shared round' >&2; exit 1; }
wait_done "$EDDA_FLEET_SCRATCH/review-pr9999-r1.done"
grep -q '^DISPATCH_EXIT=3$' "$EDDA_FLEET_SCRATCH/review-pr9999-r1.done"
out=$(launch ok 2)
printf '%s\n' "$out" | grep -qx 'review_round=2' || { printf '%s\n' "$out" >&2; echo 'launch: round 2 was not admitted through round 1 terminal receipt' >&2; exit 1; }
wait_done "$EDDA_FLEET_SCRATCH/review-pr9999-r2.done"
grep -q -- '--resume' "$EDDA_FLEET_SCRATCH/argv"

# Execute the generated Windows artifact itself. This catches Bash expanding
# PowerShell backticks in the generator, which outer sh -n cannot see.
cat >"$tmp/bin/edda.cmd" <<EOF
@echo off
if "%1"=="review" if "%2"=="--help" (echo --pr N --agent AGENT --model MODEL --json --resume & exit /b 0)
if "%ADAPTER_CASE%"=="unqualified" (
 echo {"event_id":"evt_fixture_review","subject":{"head_sha":"$sha","subject_seen":"$sha","worktree_check":"unchanged"},"reviewer":{"tool_policy":"hard","model_requested":"fixture-model","model_observed":"fixture-observed","session_id":"fixture-session"},"verdict":"lgtm","qualified":false,"disqualifiers":["gates-red","escalation-pending"],"findings":[{"severity":"P1","file":"fixture.ps1","line":12,"claim":"windows fixture","evidence":"fixture:evidence","rule":"C1","status":"open"}],"checklist":[{"item":"adapter fixture","result":"ran","measure":"fixture-measure"}],"escalations":["fixture escalation"],"cost":{"usd":0.25}}
 exit /b 3
)
echo {"event_id":"evt_fixture_review","subject":{"head_sha":"$sha","subject_seen":"$sha","worktree_check":"unchanged"},"reviewer":{"tool_policy":"hard","model_requested":"fixture-model","model_observed":"fixture-observed","session_id":"fixture-session"},"verdict":"changes-requested","qualified":true,"disqualifiers":[],"findings":[{"severity":"P1","file":"fixture.ps1","line":12,"claim":"windows fixture","evidence":"fixture:evidence","rule":"C1","status":"open"}],"checklist":[],"escalations":[],"cost":{"usd":0.25}}
exit /b 1
EOF
rm -rf "$EDDA_FLEET_SCRATCH"
ADAPTER_PLATFORM=MINGW64_NT ADAPTER_CASE=ok timeout 20 "$root/scripts/review-pr.sh" 9999 6 --dry-run >/dev/null
lane="$EDDA_FLEET_SCRATCH/review-pr9999-r6-lane.ps1"
[ -s "$lane" ] || { echo 'windows: generated lane is empty' >&2; exit 1; }
pwsh_bin=$(command -v pwsh)

# The scheduled-task launcher is generated on disk as well, so the exact
# -Argument Task Scheduler would register is provable without registering a
# task: the inline registration used to single-quote it, which made the
# backticks literal (`-File `"C:\…`"`) and every product task died with
# LastTaskResult=64 before the lane ran (#998; the #683 signature).
launch_ps1="$EDDA_FLEET_SCRATCH/review-pr9999-r6-launch.ps1"
[ -s "$launch_ps1" ] || { echo 'windows: product launcher was not generated on disk' >&2; exit 1; }
cat >"$tmp/ps-check.ps1" <<'EOF'
param([string]$Path, [string]$Mode)
$tokens = $null; $errors = $null
$ast = [System.Management.Automation.Language.Parser]::ParseFile($Path, [ref]$tokens, [ref]$errors)
if ($errors.Count -gt 0) { $errors | ForEach-Object { [Console]::Error.WriteLine($_.Message) }; exit 1 }
if ($Mode -ne 'task-argument') { exit 0 }
$cmd = $ast.Find({ param($n) $n -is [System.Management.Automation.Language.CommandAst] -and $n.GetCommandName() -eq 'New-ScheduledTaskAction' }, $true)
if (-not $cmd) { [Console]::Error.WriteLine('no New-ScheduledTaskAction in the launcher'); exit 1 }
$elements = @($cmd.CommandElements)
for ($i = 0; $i -lt $elements.Count; $i++) {
  if ($elements[$i] -is [System.Management.Automation.Language.CommandParameterAst] -and $elements[$i].ParameterName -eq 'Argument') {
    # Evaluate the literal exactly as PowerShell will when the launcher runs.
    [scriptblock]::Create($elements[$i + 1].Extent.Text).Invoke()[0]
    exit 0
  }
}
[Console]::Error.WriteLine('no -Argument on New-ScheduledTaskAction'); exit 1
EOF
ps_check() { # $1=.ps1 $2=mode
  "$pwsh_bin" -NoProfile -NonInteractive -File "$(cygpath -w "$tmp/ps-check.ps1")" -Path "$(cygpath -w "$1")" -Mode "$2"
}
for f in "$lane" "$launch_ps1"; do
  ps_check "$f" parse >/dev/null || { echo "windows: $f does not parse" >&2; exit 1; }
done
task_arg=$(ps_check "$launch_ps1" task-argument | tr -d '\r')
expected_arg="-NoProfile -NonInteractive -WindowStyle Hidden -ExecutionPolicy Bypass -File \"$(cygpath -w "$lane")\""
[ "$task_arg" = "$expected_arg" ] || {
  printf 'windows: the registered task argument would be\n  %s\nexpected\n  %s\n' "$task_arg" "$expected_arg" >&2
  exit 1
}

win_rc=0
"$pwsh_bin" -NoProfile -NonInteractive -File "$(cygpath -w "$lane")" || win_rc=$?
[ "$win_rc" = 1 ] || { echo "windows: generated lane exit=$win_rc, expected product changes-requested 1" >&2; exit 1; }
done="$EDDA_FLEET_SCRATCH/review-pr9999-r6.done"
log="$EDDA_FLEET_SCRATCH/review-pr9999-r6.log"
grep -q '^TRANSPORT=edda-review$' "$done"
grep -q '^WORKTREE_CHECK=unchanged$' "$done"
grep -q '^QUALIFIED=true$' "$done"
grep -q '"claim":"windows fixture"' "$log"
# A direct run has no scheduled task to unregister; the receipt still closes.
for line in 'FINAL_EXIT=1' 'WORKTREE_CLEANUP=removed' 'TASK_CLEANUP=not-applicable' 'TERMINAL_RECEIPT=complete'; do
  grep -qx "$line" "$done" || { cat "$done" >&2; echo "windows: product receipt lacks $line" >&2; exit 1; }
done
grep -qx -- '- model_observed: fixture-observed' "$log" || { cat "$log" >&2; echo 'windows: envelope carries no model_observed header' >&2; exit 1; }
grep -qx -- '- escalations: none' "$log" || { cat "$log" >&2; echo 'windows: envelope carries no escalations header' >&2; exit 1; }
verdict_is_last "$log" windows

# The same lane on an unqualified LGTM (exit 3): payload published, provisional
# Verdict line, no label.
ADAPTER_PLATFORM=MINGW64_NT ADAPTER_CASE=unqualified timeout 20 "$root/scripts/review-pr.sh" 9999 8 --dry-run >/dev/null
lane="$EDDA_FLEET_SCRATCH/review-pr9999-r8-lane.ps1"
win_rc=0
ADAPTER_CASE=unqualified "$pwsh_bin" -NoProfile -NonInteractive -File "$(cygpath -w "$lane")" || win_rc=$?
[ "$win_rc" = 3 ] || { echo "windows: unqualified lane exit=$win_rc, expected product exit 3" >&2; exit 1; }
done="$EDDA_FLEET_SCRATCH/review-pr9999-r8.done"
log="$EDDA_FLEET_SCRATCH/review-pr9999-r8.log"
for line in 'DISPATCH_EXIT=3' 'FINAL_EXIT=3' 'QUALIFIED=false' 'DISQUALIFIERS=gates-red,escalation-pending' 'TERMINAL_RECEIPT=complete'; do
  grep -qx "$line" "$done" || { cat "$done" >&2; echo "windows unqualified: receipt lacks $line" >&2; exit 1; }
done
grep -qx 'Provisional — unqualified (disqualifiers: gates-red, escalation-pending), P0=0, P1=1 — not a merge-gate verdict' "$log" \
  || { cat "$log" >&2; echo 'windows unqualified: the Verdict line does not state the provisional outcome' >&2; exit 1; }
grep -q '"claim":"windows fixture"' "$log" || { echo 'windows unqualified: findings were not written' >&2; exit 1; }
grep -q 'fixture escalation' "$log" || { echo 'windows unqualified: escalations were not written' >&2; exit 1; }
if grep -qE '^(LGTM|Changes Requested)' "$log"; then echo 'windows unqualified: adapter emitted a merge-gate verdict line' >&2; exit 1; fi
grep -qx -- '- escalations: fixture escalation' "$log" || { cat "$log" >&2; echo 'windows unqualified: envelope carries no escalations header' >&2; exit 1; }
verdict_is_last "$log" 'windows unqualified'
vl=$(label_of "$log")
[ -z "$vl" ] || { echo "windows unqualified: verdict-label resolved the provisional round to '$vl'" >&2; exit 1; }
printf 'review product-adapter fixtures passed (proof, qualification, provisional exit 3, detail carrier, terminal receipt, round reservation, fresh scratch, generated Windows lane and launcher)\n'
