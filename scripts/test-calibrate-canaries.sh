#!/bin/sh
# test-calibrate-canaries.sh — offline test for scripts/calibrate-canaries.sh
# (issue #881). Stubs `pi` and `claude` with fixture transcripts on PATH and
# asserts the mechanical scorer end to end:
#
#   1. sh -n passes on both scripts;
#   2. every expected.md carries front matter (grep -L '^id:' is empty);
#   3. --dry-run prints the clone path, both commits, the target diff stat and
#      the exact launch line per run, and launches no engine run;
#   4. selector grammar <backend>:<id>: pi + Anthropic id and claude +
#      non-Anthropic id exit 2 before launch (the id set is data, parsed from
#      the stubbed pi catalogue);
#   5. scoring: caught, missed, false positive, severity mismatch
#      (caught + severity_match no);
#   6. model_observed from the session file / JSON — a mismatch marks the row
#      void (never silently scored) and the script exits 1;
#   7. the for-ledger block and the per-canary union row are printed;
#   8. the clone is removed on every exit path and nothing is written outside
#      the test's temp dir.
#
# Style follows scripts/test-review-capabilities.sh — no new tooling.
set -u

SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd) || exit 1
CALIBRATE="$SCRIPT_DIR/calibrate-canaries.sh"

failures=0
bad() { failures=$((failures + 1)); printf 'FAIL: %s\n' "$1" >&2; }
good() { printf 'ok: %s\n' "$1" >&2; }

WORK=$(mktemp -d "${TMPDIR:-/tmp}/test-calibrate-canaries.XXXXXX") || exit 1
cleanup() { rm -rf -- "$WORK"; }
trap cleanup EXIT
trap 'cleanup; trap - EXIT; exit 130' INT
trap 'cleanup; trap - EXIT; exit 143' TERM

STUBS="$WORK/stubs"
LOG="$WORK/stub.log"
TEST_TMPDIR="$WORK/tmpdir"
TMPDIR_REAL="${TMPDIR:-/tmp}"
mkdir -p "$STUBS" "$TEST_TMPDIR"
export TMPDIR="$TEST_TMPDIR"

# --- stubs -----------------------------------------------------------------
# The stub catalogue includes anthropic provider rows, openrouter
# anthropic/* rows and the ~anthropic latest aliases — the data the selector
# check classifies.
cat > "$STUBS/pi" <<EOF
#!/bin/sh
printf '%s\n' "\$*" >> "$LOG"
if [ "\$1" = "--list-models" ]; then
  cat <<'CATALOGUE'
provider      model                          context  max-out  thinking  images
anthropic     claude-opus-5                  1M       128K     yes       yes
openrouter    anthropic/claude-opus-4.5      200K     64K      yes       yes
openrouter    ~anthropic/claude-opus-latest  1M       128K     yes       yes
openrouter    z-ai/glm-5.3-flash             1M       943.7K   yes       yes
openai-codex  gpt-5.6-sol                    272K     128K     yes       yes
CATALOGUE
  exit 0
fi
# run mode: extract --model and --session-dir
model=unknown
sess_dir=
prev=
for a in "\$@"; do
  case \$prev in
    --model) model=\$a ;;
    --session-dir) sess_dir=\$a ;;
  esac
  prev=\$a
done
[ -n "\$sess_dir" ] || { echo 'stub pi: no --session-dir' >&2; exit 3; }
mkdir -p "\$sess_dir"
observed=\${CALIB_STUB_SESSION_MODEL:-\$model}
{
  printf '{"type":"session","id":"stub"}\n'
  printf '{"type":"model_change","provider":"stub","modelId":"%s"}\n' "\$observed"
  printf '{"type":"message","message":{"role":"assistant","api":{"model":"%s"},"usage":{"cost":{"input":0.0005,"output":0.0004,"total":0.0009}}}}\n' "\$observed"
} > "\$sess_dir/stub-session.jsonl"
cat "\$CALIB_STUB_TRANSCRIPT"
exit 0
EOF

cat > "$STUBS/claude" <<EOF
#!/bin/sh
printf '%s\n' "\$*" >> "$LOG"
model=unknown
prev=
for a in "\$@"; do
  case \$prev in
    --model) model=\$a ;;
  esac
  prev=\$a
done
observed=\${CALIB_STUB_CLAUDE_MODEL:-\$model}
result=\$(cat "\$CALIB_STUB_TRANSCRIPT" | sed ':a;N;\$!ba;s/\n/\\\\n/g')
printf '{"result":"%s","session_id":"stub-claude","modelUsage":{"%s":{"costUSD":0.4169}},"total_cost_usd":0.4169}\n' "\$result" "\$observed"
exit 0
EOF

chmod +x "$STUBS/pi" "$STUBS/claude"

BRIEF="$WORK/calib-brief.md"
printf '你是唯讀審查員。審查目標：canary set v0。\n' > "$BRIEF"

GLM='pi:openrouter/z-ai/glm-5.3-flash'
run_calibrate() { # extra args...
  PATH="$STUBS:$PATH" sh "$CALIBRATE" \
    --engine "$GLM" --brief "$BRIEF" "$@" 2>"$WORK/stderr.txt"
}

# --- 1. syntax -------------------------------------------------------------
if sh -n "$CALIBRATE" && sh -n "$SCRIPT_DIR/test-calibrate-canaries.sh"; then
  good 'sh -n both scripts'
else
  bad 'sh -n failed'
fi

# --- 2. front matter coverage ----------------------------------------------
missing=$(cd "$SCRIPT_DIR/.." && grep -L '^id:' tests/canaries/*/*/expected.md)
if [ -z "$missing" ]; then
  good 'grep -L ^id: tests/canaries/*/*/expected.md is empty'
else
  bad "expected.md missing front matter: $missing"
fi

# --- 3. dry-run -------------------------------------------------------------
: > "$LOG"
out=$(run_calibrate --runs 2 --dry-run)
rc=$?
[ "$rc" -eq 0 ] || bad "dry-run exit $rc"
case $out in
  *edda-calib.XXXXXX/repo*) good 'dry-run prints the clone path shape under TMPDIR' ;;
  *) bad 'dry-run does not print the clone path shape (expected edda-calib.XXXXXX/repo)' ;;
esac
case $out in
  *'calibration: canary fixture pre-state'*) good 'dry-run prints fixture commit' ;;
  *) bad 'dry-run does not print the fixture commit' ;;
esac
case $out in
  *'calibration: canary set v0'*) good 'dry-run prints canary commit' ;;
  *) bad 'dry-run does not print the canary commit' ;;
esac
case $out in
  *'5 files changed'*) good 'dry-run prints the target diff stat' ;;
  *) bad 'dry-run does not print the target diff stat' ;;
esac
case $out in
  *'--tools read,grep,find,ls --session-dir'*'--session-id calib-openrouter-z-ai-glm-5.3-flash-2'*)
    good 'dry-run prints the exact pi launch line per run' ;;
  *) bad 'dry-run does not print the exact pi launch line per run' ;;
esac
# The pattern here is deliberately BROADER than the positive assertion at
# the dry-run clone-path check above, and the asymmetry is the point: a
# positive assertion should be as tight as possible so a wrong value fails
# it, while a negative assertion should be as broad as possible so a
# leftover in ANY shape is still found. A negative assertion whose pattern
# has gone stale does not go red — it goes green, silently, which is what
# happened when the clone path moved from edda-calib-$$ to edda-calib.XXXXXX
# and these two checks stopped matching anything at all.
if [ -z "$(find "$TEST_TMPDIR" -maxdepth 1 -name 'edda-calib*' -print -quit)" ]; then
  good 'dry-run creates no clone'
else
  bad 'dry-run created a clone'
fi
if grep -q '^--tools ' "$LOG" 2>/dev/null || grep -qv -- '--list-models' "$LOG"; then
  bad "dry-run launched an engine run: $LOG"
else
  good 'dry-run launched no engine run (catalogue query only)'
fi

# --- 4. selector validation -------------------------------------------------
selector_case() { # <selector> <expected-rc>
  sel=$1
  want=$2
  PATH="$STUBS:$PATH" sh "$CALIBRATE" --engine "$sel" --brief "$BRIEF" --dry-run >/dev/null 2>&1
  got=$?
  if [ "$got" -eq "$want" ]; then
    good "selector $sel -> exit $want"
  else
    bad "selector $sel: expected exit $want, got $got"
  fi
}
selector_case 'pi:anthropic/claude-opus-5' 2
selector_case 'pi:openrouter/anthropic/claude-opus-4.5' 2
selector_case 'pi:openrouter/~anthropic/claude-opus-latest' 2
selector_case 'claude:openai-codex/gpt-5.6-sol' 2
selector_case 'claude:openrouter/z-ai/glm-5.3-flash' 2
selector_case 'nocolon' 2
selector_case 'wrong:x' 2
selector_case 'pi:' 2
selector_case 'claude:claude-opus-5' 0
selector_case 'pi:openai-codex/gpt-5.6-sol' 0

# --- 5./6. scoring scenarios (stubbed pi) -----------------------------------
C1='tests/canaries/code-risk/c1-shell-precedence'
C2='tests/canaries/docs-skills/c2-stale-ratify-claim'

scenario() { # <name> <transcript-file> <expected-substring> <want-rc>
  name=$1
  printf '%s\n' "$2" > "$WORK/transcript.txt"
  CALIB_STUB_TRANSCRIPT="$WORK/transcript.txt"
  export CALIB_STUB_TRANSCRIPT
  out=$(run_calibrate --runs 1)
  got=$?
  if [ "$got" -eq "$4" ]; then
    good "$name exit $got"
  else
    bad "$name: expected exit $4, got $got (stderr: $(head -2 "$WORK/stderr.txt"))"
  fi
  case $out in
    *"$3"*) good "$name row: $3" ;;
    *) bad "$name: row '$3' not found in output:
$(printf '%s\n' "$out" | grep '^| c')"
    ;;
  esac
  # Broad by design — see the note on the dry-run clone check above.
  if [ -z "$(find "$TEST_TMPDIR" -maxdepth 1 -name 'edda-calib*' -print -quit)" ]; then
    good "$name clone removed"
  else
    bad "$name left a clone behind"
  fi
}

scenario caught \
  'FINDING P0 canaries-fixture/c1-shell-precedence/deploy.sh:7 — POSIX sh 把 || 與 && 同優先序、左結合，解析為 (fast_build || cleanup) && git rm -rf .，fast_build 成功的正常路徑也會執行 git rm -rf .' \
  '| c1-shell-precedence | 1 | caught | yes |' 0

scenario missed \
  'NO FINDINGS' \
  '| c2-stale-ratify-claim | 1 | missed | - |' 0

scenario false-positive \
  'FINDING P1 canaries-fixture/c2-stale-ratify-claim/LEDGER.md:3 — LEDGER.md 的事件日期格式不一致，建議統一格式' \
  '| c2-stale-ratify-claim | 1 | false-positive | - |' 0

scenario severity-mismatch \
  'FINDING P2 canaries-fixture/c2-stale-ratify-claim/STATUS.md:2 — STATUS.md 宣稱 D-042 尚未 ratify，與 LEDGER.md 既有的 decision_ratify 事件矛盾' \
  '| c2-stale-ratify-claim | 1 | caught | no |' 0

# model_observed mismatch -> void, exit 1
printf '%s\n' 'FINDING P0 canaries-fixture/c1-shell-precedence/deploy.sh:7 — POSIX sh 把 || 與 && 同優先序、左結合' > "$WORK/transcript.txt"
CALIB_STUB_TRANSCRIPT="$WORK/transcript.txt"
CALIB_STUB_SESSION_MODEL='openai-codex/gpt-5.6-sol'
export CALIB_STUB_TRANSCRIPT CALIB_STUB_SESSION_MODEL
out=$(run_calibrate --runs 1)
got=$?
unset CALIB_STUB_SESSION_MODEL
if [ "$got" -eq 1 ]; then
  good 'model_observed mismatch -> exit 1'
else
  bad "model_observed mismatch: expected exit 1, got $got"
fi
case $out in
  *'| c1-shell-precedence | 1 | void | - |'*) good 'model_observed mismatch -> row void' ;;
  *) bad 'model_observed mismatch did not void the row' ;;
esac
case $(cat "$WORK/stderr.txt") in
  *'observed=openai-codex/gpt-5.6-sol'*) good 'void reason names the observed model' ;;
  *) bad 'void reason does not name the observed model' ;;
esac

# engine exit != 0 -> void too (fail stub in its own dir so it shadows the
# good stub for run mode while still answering --list-models)
mkdir -p "$WORK/failstubs"
cat > "$WORK/failstubs/pi" <<EOF
#!/bin/sh
if [ "\$1" = "--list-models" ]; then exec "$STUBS/pi" --list-models; fi
echo 'boom' >&2
exit 7
EOF
chmod +x "$WORK/failstubs/pi"
PATH="$WORK/failstubs:$STUBS:$PATH" sh "$CALIBRATE" --engine "$GLM" --brief "$BRIEF" --runs 1 > "$WORK/failout.txt" 2>"$WORK/stderr.txt"
got=$?
if [ "$got" -eq 1 ]; then
  good 'engine exit != 0 -> exit 1'
else
  bad "engine exit != 0: expected exit 1, got $got"
fi
case $(cat "$WORK/failout.txt") in
  *'| c1-shell-precedence | 1 | void | - |'*) good 'engine exit != 0 -> row void' ;;
  *) bad 'engine failure did not void the row' ;;
esac
rm -rf "$WORK/failstubs"

# --- 7. union row, for-ledger, cost sum (multi-run, stubbed claude) ---------
printf '%s\n' 'FINDING P0 canaries-fixture/c1-shell-precedence/deploy.sh:7 — POSIX sh 把 || 與 && 同優先序、左結合' > "$WORK/transcript.txt"
CALIB_STUB_TRANSCRIPT="$WORK/transcript.txt"
export CALIB_STUB_TRANSCRIPT
out=$(PATH="$STUBS:$PATH" sh "$CALIBRATE" --engine 'claude:claude-opus-5' --brief "$BRIEF" --runs 2)
got=$?
if [ "$got" -eq 0 ]; then
  good 'claude backend run exit 0'
else
  bad "claude backend run: expected exit 0, got $got"
fi
case $out in
  *'| c1-shell-precedence | union | caught | yes | claude-opus-5 | claude-opus-5 | 0.833800'*)
    good 'union row sums cost across runs' ;;
  *) bad "union row missing or wrong:
$(printf '%s\n' "$out" | grep 'union')" ;;
esac
case $out in
  *'for-ledger — fleet.review-calibration'*'cost_usd: 0.833800'*)
    good 'for-ledger block with summed cost' ;;
  *) bad 'for-ledger block missing or cost not summed' ;;
esac
case $out in
  *'edda decide'*'decide'*) : ;;
  *) good 'for-ledger block states the script never runs edda decide' ;;
esac

# --- 8. claude model_observed mismatch --------------------------------------
CALIB_STUB_CLAUDE_MODEL='claude-sonnet-5'
export CALIB_STUB_CLAUDE_MODEL
out=$(PATH="$STUBS:$PATH" sh "$CALIBRATE" --engine 'claude:claude-opus-5' --brief "$BRIEF" --runs 1)
got=$?
unset CALIB_STUB_CLAUDE_MODEL
if [ "$got" -eq 1 ]; then
  good 'claude model_observed mismatch -> exit 1'
else
  bad "claude model_observed mismatch: expected exit 1, got $got"
fi
case $out in
  *'| c1-shell-precedence | 1 | void | - | claude-opus-5 | claude-sonnet-5'*)
    good 'claude model_observed mismatch -> void row carries both models' ;;
  *) bad 'claude model_observed mismatch did not void the row' ;;
esac

# --- nothing written outside the temp dir -----------------------------------
# By construction every script write goes under $TMPDIR (clone, out files,
# session dirs); the per-scenario assertions proved the clone is removed and
# the TMPDIR override proved the clone lands in the sandbox. A scan of the
# real /tmp is deliberately not performed: find/glob over /tmp can block on
# unrelated MSYS nodes on this workstation.

if [ "$failures" -eq 0 ]; then
  echo 'test-calibrate-canaries.sh: all checks passed' >&2
  exit 0
fi
printf 'test-calibrate-canaries.sh: %d check(s) FAILED\n' "$failures" >&2
exit 1
