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

cat >"$tmp/bin/uname" <<'EOF'
#!/bin/sh
echo "${ADAPTER_PLATFORM:-Linux}"
EOF
cat >"$tmp/bin/gh" <<EOF
#!/bin/sh
case "\$*" in
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
case "\${ADAPTER_CASE:-ok}" in
  malformed) printf '{not json\n'; exit 2 ;;
  changed) proof=failed; policy=hard; head='$sha' ;;
  policy) proof=unchanged; policy=none; head='$sha' ;;
  wrong-head) proof=unchanged; policy=hard; head=cccccccccccccccccccccccccccccccccccccccc ;;
  unqualified) proof=unchanged; policy=hard; head='$sha'; verdict=lgtm; qualified=false; disqualifiers='["gates-red","escalation-pending"]'; exit_code=3 ;;
  detailed) proof=unchanged; policy=hard; head='$sha'; verdict=changes-requested; qualified=true; disqualifiers='[]'; exit_code=1 ;;
  *) proof=unchanged; policy=hard; head='$sha'; verdict=changes-requested; qualified=true; disqualifiers='[]'; exit_code=1 ;;
esac
printf '{"event_id":"evt_fixture_review","subject":{"head_sha":"%s","subject_seen":"%s","worktree_check":"%s"},"reviewer":{"tool_policy":"%s","model_requested":"fixture-model","model_observed":"fixture-observed","session_id":"fixture-session"},"verdict":"%s","qualified":%s,"disqualifiers":%s,"findings":[{"severity":"P1","file":"scripts/review-pr.sh","line":261,"claim":"fixture claim","evidence":"fixture:evidence","rule":"C1","status":"open"}],"checklist":[{"item":"adapter fixture","result":"ran","measure":"fixture-measure"}],"escalations":["fixture escalation"],"cost":{"usd":0.25}}\n' "\$head" "\$head" "\$proof" "\$policy" "\${verdict:-changes-requested}" "\${qualified:-true}" "\${disqualifiers:-[]}"
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

run_case ok unchanged 1
done="$EDDA_FLEET_SCRATCH/review-pr9999-r1.done"
log="$EDDA_FLEET_SCRATCH/review-pr9999-r1.log"
grep -q '^TRANSPORT=edda-review$' "$done"
grep -q '^POLICY_RECEIPT=product-json:hard$' "$done"
grep -q '^Changes Requested, P0=0, P1=1$' "$log"
if grep -q '^TOOL_FLAGS=' "$done"; then echo 'ok: legacy policy string was fabricated' >&2; exit 1; fi
grep -q 'nohup "\$RUNNER"' "$root/scripts/review-pr.sh"

for case_name in changed policy wrong-head malformed; do run_case "$case_name" 'failed;' 1; done
ADAPTER_CASE=ok run_case ok unchanged 2
grep -q -- '--resume' "$EDDA_FLEET_SCRATCH/argv"
grep -q -- '--agent claude --model claude-opus-5 --json --resume' "$EDDA_FLEET_SCRATCH/argv"

# An unqualified LGTM has product exit 3. It must reach the adapter receipt as
# such and must never manufacture the marker the watcher maps to review:lgtm.
run_case unqualified unchanged 3
done="$EDDA_FLEET_SCRATCH/review-pr9999-r3.done"
log="$EDDA_FLEET_SCRATCH/review-pr9999-r3.log"
grep -q '^DISPATCH_EXIT=3$' "$done"
grep -q '^QUALIFIED=false$' "$done"
grep -q '^DISQUALIFIERS=gates-red,escalation-pending$' "$done"
if grep -q '^<<<VERDICT$' "$log"; then echo 'unqualified: adapter emitted an approval-capable verdict envelope' >&2; exit 1; fi

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

# First use starts without the state directory. The selected product path owns
# creation before it writes its lane/runner artifacts.
rm -rf "$EDDA_FLEET_SCRATCH"
ADAPTER_CASE=ok timeout 20 "$root/scripts/review-pr.sh" 9999 5 --dry-run >/dev/null
[ -d "$EDDA_FLEET_SCRATCH" ] || { echo 'fresh scratch: product adapter did not create its state directory' >&2; exit 1; }
[ -s "$EDDA_FLEET_SCRATCH/review-pr9999-r5-run.sh" ] || { echo 'fresh scratch: product runner was not generated' >&2; exit 1; }

# Execute the generated Windows artifact itself. This catches Bash expanding
# PowerShell backticks in the generator, which outer sh -n cannot see.
cat >"$tmp/bin/edda.cmd" <<EOF
@echo off
if "%1"=="review" if "%2"=="--help" (echo --pr N --agent AGENT --model MODEL --json --resume & exit /b 0)
echo {"event_id":"evt_fixture_review","subject":{"head_sha":"$sha","subject_seen":"$sha","worktree_check":"unchanged"},"reviewer":{"tool_policy":"hard","model_requested":"fixture-model","model_observed":"fixture-observed","session_id":"fixture-session"},"verdict":"changes-requested","qualified":true,"disqualifiers":[],"findings":[{"severity":"P1","file":"fixture.ps1","line":12,"claim":"windows fixture","evidence":"fixture:evidence","rule":"C1","status":"open"}],"checklist":[],"escalations":[],"cost":{"usd":0.25}}
exit /b 1
EOF
rm -rf "$EDDA_FLEET_SCRATCH"
ADAPTER_PLATFORM=MINGW64_NT ADAPTER_CASE=ok timeout 20 "$root/scripts/review-pr.sh" 9999 6 --dry-run >/dev/null
lane="$EDDA_FLEET_SCRATCH/review-pr9999-r6-lane.ps1"
[ -s "$lane" ] || { echo 'windows: generated lane is empty' >&2; exit 1; }
win_rc=0
"$(command -v pwsh)" -NoProfile -NonInteractive -File "$(cygpath -w "$lane")" || win_rc=$?
[ "$win_rc" = 1 ] || { echo "windows: generated lane exit=$win_rc, expected product changes-requested 1" >&2; exit 1; }
done="$EDDA_FLEET_SCRATCH/review-pr9999-r6.done"
log="$EDDA_FLEET_SCRATCH/review-pr9999-r6.log"
grep -q '^TRANSPORT=edda-review$' "$done"
grep -q '^WORKTREE_CHECK=unchanged$' "$done"
grep -q '^QUALIFIED=True$' "$done"
grep -q '"claim":"windows fixture"' "$log"
printf 'review product-adapter fixtures passed (proof, qualification, detail carrier, fresh scratch, generated Windows lane)\n'
