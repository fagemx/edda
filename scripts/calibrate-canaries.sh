#!/bin/sh
# calibrate-canaries.sh — the tests/canaries/README.md §「如何跑一次校準」
# recipe as a script (issue #881; design #618 §1.2 and §7 item 7).
#
# For each requested engine (<backend>:<catalogue id>, id copied verbatim) it
# builds a throwaway clone under ${TMPDIR:-/tmp}/edda-calib-$$, commits the
# canary fixture pre-state, then commits the canary set (two commits, exactly
# the README recipe), produces the target diff HEAD~1..HEAD, launches the
# engine READ-ONLY once per run with cwd = the clone, collects model_observed
# from the system (pi session file "model" field; claude --output-format json
# "modelUsage" key — never from the transcript's own text, #616), scores each
# canary mechanically against its expected.md front matter, and prints a
# Markdown results table plus a for-ledger block. The clone is removed on
# every exit path (trap on EXIT/INT/TERM).
#
# The script is calibration plumbing only: it never posts to GitHub, never
# writes to the ledger and never runs `edda decide` — it prints the for-ledger
# block; the controller records it (design §3: 本 lane 不執行 edda decide).
#
# Launch lines follow the review transport conventions (README recipe and
# scripts/review-pr.sh): pi is read-only via the --tools read,grep,find,ls
# allowlist; claude via the review allowlist
# --allowedTools "Read,Grep,Glob,Bash(git *),Bash(sh *)".
# Opus / Anthropic ids are never sent through pi
# (fleet.claude-subscription-transport=claude-code-only): the check is data —
# the id set is parsed out of the pi catalogue (`pi --list-models`), not
# hard-coded (design §3 learning 3: ids are matched, never fuzzy-resolved).
#
# Scoring (mechanical, per tests/canaries/<class>/<name>/expected.md front
# matter keys id/class/severity/file/match):
#   caught          a FINDING line on the expected file whose text matches
#                   the `match` regex (ERE);
#   false-positive  no such finding, but a FINDING line on the canary's
#                   surface (the expected file or anything under its
#                   directory) that is not the expected finding;
#   missed          otherwise.
# severity_match compares the caught finding's reported severity with the
# front-matter severity. A run whose engine exit ≠ 0 or whose model_observed
# differs from model_requested marks every row of that run `void` — never
# silently scored (model_requested ≠ model_observed is a P0 incident).
# The mechanical score is conservative: human scoring per the expected.md
# 評分提示 remains the authority for qualification decisions.
#
# usage: calibrate-canaries.sh --engine <backend>:<id> [--engine ...]
#                              --brief <path> [--runs N] [--dry-run]
#
# exit: 0 all runs scored clean; 1 any run void/failed (all runs still
#       complete); 2 usage / selector / expected.md front-matter errors —
#       always before any engine launch.
set -u

usage() {
  cat >&2 <<'EOF'
usage: calibrate-canaries.sh --engine <backend>:<id> [--engine ...]
                             --brief <path> [--runs N] [--dry-run]
  --engine   repeatable; <backend> is `pi` or `claude`; <id> is a catalogue
             id copied verbatim (e.g. pi:openrouter/z-ai/glm-5.3-flash)
  --brief    reviewer brief path (reviewer-brief-template-v1 filled); the
             script appends the mechanical-scoring output protocol to it
  --runs     runs per engine (default 1)
  --dry-run  print the clone path, the two commits, the target diff stat and
             the exact launch line per run; launch nothing
EOF
}

die() { # die <code> <message...>
  code=$1; shift
  printf 'calibrate-canaries.sh: %s\n' "$*" >&2
  exit "$code"
}

ENGINES=''
BRIEF=''
RUNS=1
DRY_RUN=0

while [ $# -gt 0 ]; do
  case $1 in
    --engine)
      [ $# -ge 2 ] || { usage; exit 2; }
      ENGINES="$ENGINES
$2"
      shift 2 ;;
    --engine=*)
      ENGINES="$ENGINES
${1#*=}"
      shift ;;
    --brief)
      [ $# -ge 2 ] || { usage; exit 2; }
      BRIEF=$2
      shift 2 ;;
    --brief=*)
      BRIEF=${1#*=}
      shift ;;
    --runs)
      [ $# -ge 2 ] || { usage; exit 2; }
      RUNS=$2
      shift 2 ;;
    --runs=*)
      RUNS=${1#*=}
      shift ;;
    --dry-run)
      DRY_RUN=1
      shift ;;
    -h|--help)
      usage
      exit 0 ;;
    *)
      usage
      exit 2 ;;
  esac
done

[ -n "$BRIEF" ] || { usage; exit 2; }
[ -f "$BRIEF" ] && [ -r "$BRIEF" ] || die 2 "brief not readable: $BRIEF"
case $RUNS in
  ''|*[!0-9]*) die 2 "--runs must be a positive integer: $RUNS" ;;
esac
[ "$RUNS" -ge 1 ] || die 2 "--runs must be a positive integer: $RUNS"
ENGINES=${ENGINES#?
}
[ -n "$ENGINES" ] || { usage; exit 2; }

SELF_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd) || exit 2
WT=$(CDPATH= cd -- "$SELF_DIR/.." && pwd) || exit 2
CANARIES_DIR="$WT/tests/canaries"
[ -d "$CANARIES_DIR" ] || die 2 "canary set not found: $CANARIES_DIR"

# ---------------------------------------------------------------------------
# Discover canaries and validate the expected.md front matter (wiring audit
# row 1: a missing key is a hard exit 2 naming the file — never a silent
# "missed").
# ---------------------------------------------------------------------------
front_matter_field() { # <expected.md> <key> -> value
  # Single awk process reading the file directly: a `sed | ... | head -n 1`
  # pipeline has an early-exit reader and is prone to MSYS/Git Bash pipe
  # deadlocks (observed as a hang in the validation loop on Windows).
  awk -v key="$2" '
    NR == 1 { next }
    /^---[[:space:]]*$/ { exit }
    index($0, key ":") == 1 {
      v = $0
      sub("^" key ":[[:space:]]*", "", v)
      sub(/\r$/, "", v)
      if (v ~ /^\047/) { sub(/^\047/, "", v); sub(/\047$/, "", v) }
      else if (v ~ /^\042/) { sub(/^\042/, "", v); sub(/\042$/, "", v) }
      print v
      exit
    }' "$1"
}

CANARIES=$(cd "$CANARIES_DIR" && ls -d */*/ 2>/dev/null | sed 's/\/$//;s/\r$//' | sort) || CANARIES=''
[ -n "$CANARIES" ] || die 2 "no canary directories under $CANARIES_DIR"

for cdir in $CANARIES; do
  exp="$CANARIES_DIR/$cdir/expected.md"
  [ -f "$exp" ] || die 2 "missing expected.md: $exp"
  [ "$(sed -n '1{s/[[:space:]]*$//;p}' "$exp" | tr -d '\r')" = '---' ] \
    || die 2 "expected.md without front matter (line 1 must be ---): $exp"
  for key in id class severity file match; do
    val=$(front_matter_field "$exp" "$key")
    [ -n "$val" ] || die 2 "expected.md front matter missing key '$key': $exp"
  done
  sev=$(front_matter_field "$exp" severity)
  case $sev in
    P[0-3]) ;;
    *) die 2 "expected.md severity must be P0..P3, got '$sev': $exp" ;;
  esac
done

# ---------------------------------------------------------------------------
# Engine selector validation — data, not vibes: the Anthropic-id set is
# parsed from the pi catalogue. `claude:` + non-Anthropic id and `pi:` +
# Anthropic id exit 2 before any launch
# (fleet.claude-subscription-transport=claude-code-only).
# ---------------------------------------------------------------------------
load_catalogue() {
  if command -v pi >/dev/null 2>&1; then
    pi --list-models 2>/dev/null
  elif command -v edda >/dev/null 2>&1; then
    edda dispatch --agent pi --list-models 2>/dev/null
  else
    return 1
  fi
}

CATALOGUE=$(load_catalogue) \
  || die 2 "cannot validate engine selectors: pi catalogue unavailable (install pi or edda)"

# rows: "<A|N> <pi selector id>" — A = Anthropic (pi transport forbidden),
# N = non-Anthropic (claude transport forbidden).
ID_CLASSES=$(printf '%s\n' "$CATALOGUE" | awk '
  NF >= 2 && $1 != "provider" {
    if ($1 == "anthropic") { print "A " $2; print "A anthropic/" $2 }
    else if ($2 ~ /^~?anthropic\//) { print "A " $1 "/" $2 }
    else print "N " $1 "/" $2
  }')

id_class() { # <id> -> A | N | ''
  printf '%s\n' "$ID_CLASSES" | awk -v id="$1" '$2 == id { print $1; exit }'
}

sanitize() { # engine id -> session-id-safe token
  printf '%s' "$1" | sed 's/[^A-Za-z0-9._-]/-/g'
}

VALIDATED=''
for sel in $ENGINES; do
  case $sel in
    *:*) backend=${sel%%:*}; id=${sel#*:} ;;
    *) die 2 "engine selector grammar is <backend>:<catalogue id>, got: $sel" ;;
  esac
  case $backend in
    pi|claude) ;;
    *) die 2 "unknown backend '$backend' in selector $sel (pi|claude)" ;;
  esac
  [ -n "$id" ] || die 2 "empty catalogue id in selector $sel"
  case $id in
    *:*) die 2 "catalogue id must not contain ':' (copy the id verbatim from pi --list-models): $id" ;;
  esac
  cls=$(id_class "$id")
  if [ "$backend" = pi ] && [ "$cls" = A ]; then
    die 2 "refusing pi transport for Anthropic id '$id' (fleet.claude-subscription-transport=claude-code-only)"
  fi
  if [ "$backend" = claude ] && [ "$cls" = N ]; then
    die 2 "refusing claude transport for non-Anthropic id '$id' (fleet.claude-subscription-transport=claude-code-only)"
  fi
  VALIDATED="$VALIDATED
$backend|$id"
done
VALIDATED=${VALIDATED#?
}

# ---------------------------------------------------------------------------
# Launch-line builders (single source of truth for --dry-run and real runs).
# ---------------------------------------------------------------------------
pi_launch_line() { # <id> <san> <run> <clone>
  printf '(cd "%s" && pi -p --model %s --tools read,grep,find,ls --session-dir "%s/sessions/r%s" --session-id calib-%s-%s "$(cat calib-brief.md)")' \
    "$4" "$1" "$4" "$3" "$2" "$3"
}

claude_launch_line() { # <id> <san> <run> <clone>
  printf '(cd "%s" && claude -p --model %s --allowedTools "Read,Grep,Glob,Bash(git *),Bash(sh *)" --output-format json < calib-brief.md)' \
    "$4" "$1"
}

FIXTURE_COMMIT_MSG='calibration: canary fixture pre-state'
CANARY_COMMIT_MSG='calibration: canary set v0'
PATCHES=''
for cdir in $CANARIES; do
  PATCHES="$PATCHES
$CANARIES_DIR/$cdir/diff.patch"
done
PATCHES=${PATCHES#?
}

DRY_TARGET_STAT=''
for p in $PATCHES; do
  [ -f "$p" ] || die 2 "missing canary patch: $p"
  DRY_TARGET_STAT="$DRY_TARGET_STAT$(cat "$p")
"
done
DRY_TARGET_STAT=$(printf '%s' "$DRY_TARGET_STAT" | git apply --stat - 2>/dev/null)

# ---------------------------------------------------------------------------
# Dry run: describe everything, launch nothing (not even a review run).
# ---------------------------------------------------------------------------
if [ "$DRY_RUN" -eq 1 ]; then
  echo "clone: ${TMPDIR:-/tmp}/edda-calib-$$  (branch calib-canary-v0, from origin/main)"
  echo
  echo "commit 1: \"$FIXTURE_COMMIT_MSG\""
  for cdir in $CANARIES; do
    if [ -d "$CANARIES_DIR/$cdir/fixture" ]; then
      echo "  cp -r tests/canaries/$cdir/fixture -> canaries-fixture/$(basename "$cdir")"
    fi
  done
  echo "commit 2: \"$CANARY_COMMIT_MSG\""
  for cdir in $CANARIES; do
    echo "  git apply tests/canaries/$cdir/diff.patch"
  done
  echo
  echo "target diff (HEAD~1..HEAD) stat:"
  printf '%s\n' "$DRY_TARGET_STAT"
  echo
  echo "brief: $BRIEF (a fixed output-protocol addendum for mechanical scoring is appended)"
  echo
  for pair in $VALIDATED; do
    backend=${pair%%|*}
    id=${pair#*|}
    san=$(sanitize "$id")
    run=1
    while [ "$run" -le "$RUNS" ]; do
      if [ "$backend" = pi ]; then
        pi_launch_line "$id" "$san" "$run" '${TMPDIR:-/tmp}/edda-calib-'"$$"
      else
        claude_launch_line "$id" "$san" "$run" '${TMPDIR:-/tmp}/edda-calib-'"$$"
      fi
      echo "  ^ run $run/$RUNS for $backend:$id"
      run=$((run + 1))
    done
  done
  exit 0
fi

# ---------------------------------------------------------------------------
# Real run: throwaway clone, two commits, target diff, per-run launches.
# ---------------------------------------------------------------------------
CLONE="${TMPDIR:-/tmp}/edda-calib-$$"
cleanup() { rm -rf -- "$CLONE"; }
trap cleanup EXIT
trap 'cleanup; trap - EXIT; exit 130' INT
trap 'cleanup; trap - EXIT; exit 143' TERM

git clone --quiet "$WT" "$CLONE" \
  || die 2 "git clone $WT -> $CLONE failed"
cd "$CLONE" || die 2 "cd $CLONE failed"
git checkout -q -B calib-canary-v0 origin/main 2>/dev/null \
  || git checkout -q -B calib-canary-v0 \
  || die 2 "checkout calib-canary-v0 failed"

# Commit 1 — fixture pre-state (fact sources land before the review target).
mkdir -p canaries-fixture
for cdir in $CANARIES; do
  if [ -d "$CANARIES_DIR/$cdir/fixture" ]; then
    cp -r "$CANARIES_DIR/$cdir/fixture" "canaries-fixture/$(basename "$cdir")"
  fi
done
if [ -n "$(git status --porcelain -- canaries-fixture)" ]; then
  git add canaries-fixture
  git commit -q -m "$FIXTURE_COMMIT_MSG" || die 2 "fixture pre-state commit failed"
fi

# Commit 2 — the canary set (the review target).
for cdir in $CANARIES; do
  git apply "$CANARIES_DIR/$cdir/diff.patch" \
    || die 2 "git apply failed for $cdir"
done
git add -A
git commit -q -m "$CANARY_COMMIT_MSG" || die 2 "canary commit failed"

echo "clone: $CLONE"
echo "target diff (HEAD~1..HEAD) stat:"
git diff --stat HEAD~1..HEAD
echo

# Brief = the supplied brief verbatim + the output-protocol addendum that the
# mechanical scorer parses (FINDING lines; everything else is narrative).
{
  cat "$BRIEF"
  cat <<'EOF'

## 輸出協定（校準評分——機械解析，務必遵守）

每個 finding 用一行固定格式輸出（其餘敘述不會被計分）：

FINDING P<n> <repo 相對路徑> — <一行 finding 描述>

- P<n> 是你對該 finding 的 severity 判定（P0/P1/P2）。
- <repo 相對路徑> 指向證據所在檔案（可帶 :行號）。
- 沒有 finding 時，輸出一行：NO FINDINGS
EOF
} > calib-brief.md


any_void=0

score_run() { # <transcript> -> ROW lines (canary result severity_match) on stdout
  transcript=$1
  findings_file=$1.findings
  sed -n -E 's/^FINDING[[:space:]]+(P[0-3])[[:space:]]+([^[:space:]]+)[[:space:]]+(-|—)[[:space:]]+(.*)$/\1\t\2\t\4/p' \
    "$transcript" > "$findings_file" 2>/dev/null

  for cdir in $CANARIES; do
    exp="$CANARIES_DIR/$cdir/expected.md"
    cid=$(front_matter_field "$exp" id)
    sev=$(front_matter_field "$exp" severity)
    file=$(front_matter_field "$exp" file)
    regex=$(front_matter_field "$exp" match)
    exp_dir=$(dirname "$file")

    caught=0
    sev_ok=0
    on_surface=0
    while IFS="$(printf '\t')" read -r fsev fpath ftext; do
      [ -n "$fpath" ] || continue
      ffile=${fpath%%:*}
      case $ffile in
        "$exp_dir"/*) on_surface=1 ;;
      esac
      case $ffile in
        "$file")
          if printf '%s\n' "$ftext" | grep -Eq -- "$regex" 2>/dev/null; then
            caught=1
            [ "$fsev" = "$sev" ] && sev_ok=1
          else
            on_surface=1
          fi
          ;;
      esac
    done < "$findings_file"

    if [ "$caught" -eq 1 ]; then
      result=caught
      [ "$sev_ok" -eq 1 ] && smatch=yes || smatch=no
    elif [ "$on_surface" -eq 1 ]; then
      result=false-positive
      smatch=-
    else
      result=missed
      smatch=-
    fi
    printf 'ROW %s %s %s\n' "$cid" "$result" "$smatch"
  done
  rm -f "$findings_file"
}

for pair in $VALIDATED; do
  backend=${pair%%|*}
  id=${pair#*|}
  san=$(sanitize "$id")
  echo "## engine $backend:$id"
  echo
  echo '| canary | run | result | severity_match | model_requested | model_observed | cost_usd |'
  echo '|---|---|---|---|---|---|---|'

  union_file="$CLONE/union-$san"
  : > "$union_file"
  costs=''
  last_observed=unknown
  last_cost='-'

  run=1
  while [ "$run" -le "$RUNS" ]; do
    out="$CLONE/out-$san-$run.txt"
    sess_dir="$CLONE/sessions/r$run"
    rm -rf "$sess_dir"

    if [ "$backend" = pi ]; then
      eval "$(pi_launch_line "$id" "$san" "$run" "$CLONE")" > "$out" 2>"$out.err"
    else
      eval "$(claude_launch_line "$id" "$san" "$run" "$CLONE")" > "$out.json" 2>"$out.err"
    fi
    exit_code=$?

    # model_observed comes from the system — pi session file "model" field /
    # claude JSON envelope modelUsage key — never from the transcript's own
    # text (#616). A mismatch voids the run instead of being silently scored.
    observed=unknown
    cost=''
    if [ "$backend" = pi ]; then
      sess_file=$(ls -t "$sess_dir"/*.jsonl 2>/dev/null | head -n 1)
      if [ -n "${sess_file:-}" ] && [ -f "$sess_file" ]; then
        observed=$(grep -o '"model":"[^"]*"' "$sess_file" | tail -n 1 | sed 's/.*:"//;s/"$//')
        [ -n "$observed" ] \
          || observed=$(grep -o '"modelId":"[^"]*"' "$sess_file" | tail -n 1 | sed 's/.*:"//;s/"$//')
        cost=$(grep -o '"cost":{[^}]*}' "$sess_file" | tail -n 1 \
          | sed -n 's/.*"total":\([0-9.e+-]*\).*/\1/p')
      fi
    else
      if command -v jq >/dev/null 2>&1; then
        observed=$(jq -r '.modelUsage | keys[0] // "unknown"' "$out.json" 2>/dev/null)
        cost=$(jq -r '.total_cost_usd // empty' "$out.json" 2>/dev/null)
        jq -r '.result // empty' "$out.json" 2>/dev/null > "$out"
      else
        echo "calibrate-canaries.sh: jq not found on PATH — cannot read claude modelUsage/cost; run voided" >&2
      fi
    fi
    [ -n "$observed" ] || observed=unknown

    if [ "$exit_code" -ne 0 ]; then
      any_void=1
      reason="engine exit $exit_code"
    elif [ "$observed" != "$id" ]; then
      any_void=1
      reason='model_observed mismatch (model_requested ≠ model_observed is a P0 incident)'
    else
      reason=''
    fi

    rows_file="$CLONE/rows-$san-$run"
    if [ -n "$reason" ]; then
      for cdir in $CANARIES; do
        cid=$(front_matter_field "$CANARIES_DIR/$cdir/expected.md" id)
        printf 'ROW %s void -\n' "$cid"
      done > "$rows_file"
      printf 'calibrate-canaries.sh: run void — %s:%s run %s (%s; requested=%s observed=%s)\n' \
        "$backend" "$id" "$run" "$reason" "$id" "$observed" >&2
    else
      score_run "$out" > "$rows_file"
    fi

    if [ -n "$cost" ]; then costcell=$cost; else costcell='-'; fi
    while IFS="$(printf ' 	')" read -r _tag cid result smatch; do
      [ -n "$cid" ] || continue
      printf '| %s | %s | %s | %s | %s | %s | %s |\n' \
        "$cid" "$run" "$result" "$smatch" "$id" "$observed" "$costcell"
      printf '%s %s %s %s\n' "$cid" "$result" "$smatch" "$costcell" >> "$union_file"
    done < "$rows_file"
    rm -f "$rows_file"

    case $cost in
      ''|*[!0-9.]*) ;;  # not a plain number — excluded from the sum
      *) costs="$costs
$cost" ;;
    esac
    last_observed=$observed
    last_cost=$costcell
    run=$((run + 1))
  done

  # Union row per canary across this engine's runs: caught beats
  # false-positive beats missed; severity_match yes iff every caught run
  # reported the expected severity.
  for cdir in $CANARIES; do
    exp="$CANARIES_DIR/$cdir/expected.md"
    cid=$(front_matter_field "$exp" id)
    ucaught=0; ufp=0; uallmatch=1
    while IFS="$(printf ' \t')" read -r _cid uresult usmatch _ucost; do
      [ -n "$uresult" ] || continue
      case $uresult in
        caught) ucaught=1; [ "$usmatch" = yes ] || uallmatch=0 ;;
        false-positive) ufp=1 ;;
      esac
    done <<UNIONEOF
$(grep "^$cid " "$union_file")
UNIONEOF
    if [ "$ucaught" -eq 1 ]; then
      uresult=caught
      [ "$uallmatch" -eq 1 ] && usmatch=yes || usmatch=no
    elif [ "$ufp" -eq 1 ]; then
      uresult=false-positive
      usmatch=-
    else
      uresult=missed
      usmatch=-
    fi
    usum=$(grep "^$cid " "$union_file" | awk '$4 ~ /^[0-9.]+$/ {s+=$4} END {if (s != "") printf "%.6f", s}')
    [ -n "$usum" ] || usum='-'
    printf '| %s | union | %s | %s | %s | %s | %s |\n' \
      "$cid" "$uresult" "$usmatch" "$id" "$last_observed" "$usum"
  done
  rm -f "$union_file"

  # for-ledger block — the controller records it; this script never runs
  # `edda decide`, never writes the ledger, never posts to GitHub.
  usum=$(printf '%s\n' "$costs" | awk 'NF && $1 ~ /^[0-9.]+$/ {s+=$1} END {if (s != "") printf "%.6f", s; else print "-"}')
  echo
  echo '## for-ledger — fleet.review-calibration'
  echo
  echo '本腳本不執行 edda decide、不寫帳本、不發 GitHub 請求；以下區塊由操作者逐字記帳本。'
  echo
  echo "engine: $backend:$id"
  echo "requested: $id"
  echo "observed: $last_observed"
  echo "cost_usd: $usum"
  echo "runs: $RUNS (void runs are marked void in the table)"
  echo
done

if [ "$any_void" -eq 1 ]; then
  exit 1
fi
exit 0
