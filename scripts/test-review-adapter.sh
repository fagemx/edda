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
echo Linux
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
  *) proof=unchanged; policy=hard; head='$sha' ;;
esac
printf '{"subject":{"head_sha":"%s","subject_seen":"%s","worktree_check":"%s"},"reviewer":{"tool_policy":"%s","model_requested":"fixture-model","model_observed":"fixture-observed","session_id":"fixture-session"},"verdict":"changes-requested","findings":[{"severity":"P1"}],"cost":{"usd":0.25}}\n' "\$head" "\$head" "\$proof" "\$policy"
exit 1
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
printf 'review product-adapter fixtures passed (success, changed-proof, policy, head, malformed, resume)\n'
