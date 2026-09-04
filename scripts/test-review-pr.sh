#!/bin/sh
# Offline fixtures for scripts/review-pr.sh — the four defects of #683, #691
# and #697:
#
#   D1 (#683) the scheduled task's -File argument must be a Windows path, not
#             the raw POSIX $SCRATCH Git Bash hands out (LastTaskResult=64);
#   D2 (#683) the linked issue's doneWhen must survive a body that says
#             "Closes #N" instead of "Issue: #N", and its absence must be
#             stated in the brief instead of silently dropping the ceiling;
#   D3 (#691) the generated brief must never tell a reviewer to run
#             `git <verb> --help`, which opens the HTML manual in the
#             operator's browser on Windows;
#   D4 (#697) verdict-label must read the Verdict line, not the last verdict
#             keyword anywhere in the prose.
#   D5 (#708) the review transport is `edda dispatch --agent claude` with the
#             model explicitly pinned (--model) and a valid-UUID session id —
#             the claude backend rejects non-UUID session ids — and the
#             generated lane is written even on --dry-run so the transport is
#             inspectable offline before anything launches;
#   D6 (#708) EDDA_REVIEW_MODEL flows into the lane's --model verbatim;
#   D7 (#708) the spec's closing-keyword rule is narrowed to
#             pr.closing-keyword=only-when-all-donewhen-delivered (#699) —
#             no blanket ban.
#
# Everything is offline: EDDA_FLEET_SCRATCH is a temp dir, EDDA_REVIEW_SPEC
# points at the checkout's REVIEW.md (read, never written), and `gh`, `uname`
# and `cygpath` are stubs — stubbing the last two is what lets the Windows-only
# D1 path be asserted from any platform. The real ~/.edda/fleet is compared
# before and after a full run (guarded below).
# Style follows scripts/test-pr-review-watch.sh — no new tooling.
set -eu

root=$(CDPATH= cd -- "$(dirname "$0")/.." && pwd)
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' 0 HUP INT TERM

REAL_SCRATCH="$HOME/.edda/fleet"
listing_before="$tmp/real-scratch-before"
listing_after="$tmp/real-scratch-after"
ls -A "$REAL_SCRATCH" >"$listing_before" 2>/dev/null || : >"$listing_before"

case_number=0
failures=0
FIXTURE_PR=9999   # a PR number no real lane uses, so the offline guard is exact

# --- offline guarantees -------------------------------------------------------
export EDDA_FLEET_SCRATCH="$tmp/scratch"
export EDDA_REVIEW_SPEC="$root/REVIEW.md"
export EDDA_REPO="fagemx/edda"
mkdir -p "$EDDA_FLEET_SCRATCH"

# --- stubs: gh, uname, cygpath ------------------------------------------------
STUBBIN="$tmp/bin"
mkdir -p "$STUBBIN"

# gh: every read review-pr.sh makes, driven by GH_* files in the temp dir.
cat >"$STUBBIN/gh" <<'EOF'
#!/bin/sh
echo "gh $*" >>"$GH_STUB_LOG"
case "$1" in
  pr)
    case "$2" in
      view)
        case "$*" in
          *headRefOid*)   echo "${GH_HEAD:-0000000000000000000000000000000000000000}" ;;
          *headRefName*)  echo "${GH_BRANCH:-fix/example}" ;;
          *baseRefName*)  echo "${GH_BASE_REF:-main}" ;;
          *baseRefOid*)   echo "${GH_BASE_SHA:-1111111111111111111111111111111111111111}" ;;
          *title*)        echo "${GH_TITLE:-a title}" ;;
          *closingIssuesReferences*)
                          [ -n "${GH_CLOSING_FILE:-}" ] && cat "$GH_CLOSING_FILE"
                          ;;
          *body*)         [ -n "${GH_BODY_FILE:-}" ] && cat "$GH_BODY_FILE" ;;
        esac
        exit 0
        ;;
      diff)
        [ -n "${GH_FILES_FILE:-}" ] && cat "$GH_FILES_FILE"
        exit 0
        ;;
    esac
    exit 0
    ;;
  issue)
    # gh issue view <N> --repo R --json body --jq .body
    n=$3
    if [ -f "$GH_ISSUE_DIR/$n" ]; then cat "$GH_ISSUE_DIR/$n"; else exit 1; fi
    exit 0
    ;;
esac
exit 0
EOF

# uname: force the Windows branch so the D1 path is assertable everywhere.
cat >"$STUBBIN/uname" <<'EOF'
#!/bin/sh
echo "MINGW64_NT-10.0-26200"
EOF

# cygpath -w: /c/foo/bar -> C:\foo\bar. Like the real one, every absolute POSIX
# path comes back drive-rooted — a mount point such as /tmp included, which is
# why the fallback arm still prefixes a drive rather than returning \tmp\...
cat >"$STUBBIN/cygpath" <<'EOF'
#!/bin/sh
p=$2
case "$p" in
  /[a-zA-Z]/*)
    d=$(printf '%s' "$p" | cut -c2 | tr 'a-z' 'A-Z')
    rest=$(printf '%s' "$p" | cut -c3- | tr '/' '\\')
    printf '%s:%s\n' "$d" "$rest"
    ;;
  /*) printf 'C:%s\n' "$(printf '%s' "$p" | tr '/' '\\')" ;;
  *)  printf '%s\n' "$(printf '%s' "$p" | tr '/' '\\')" ;;
esac
EOF

chmod +x "$STUBBIN/gh" "$STUBBIN/uname" "$STUBBIN/cygpath"

export GH_STUB_LOG="$tmp/gh-stub.log"
export GH_ISSUE_DIR="$tmp/issues"
mkdir -p "$GH_ISSUE_DIR"
: >"$GH_STUB_LOG"
export PATH="$STUBBIN:$PATH"

# --- fixtures -----------------------------------------------------------------
printf '%s\n' 'scripts/review-pr.sh' >"$tmp/files"
export GH_FILES_FILE="$tmp/files"
export GH_HEAD="aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
export GH_BASE_SHA="bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"

cat >"$GH_ISSUE_DIR/650" <<'EOF'
Some preamble about the defect.

## doneWhen

- the launcher reaches Running from Git Bash
- `sh -n scripts/review-pr.sh` exits 0

## Relation

nothing
EOF

fail() { printf 'FAIL %s\n' "$1" >&2; failures=$((failures + 1)); }

# Run one --dry-run and leave stdout in $out, the brief path in $brief.
dry_run() { # $1 = body text
    case_number=$((case_number + 1))
    printf '%b' "$1" >"$tmp/body"
    export GH_BODY_FILE="$tmp/body"
    rm -f "$EDDA_FLEET_SCRATCH/review-pr$FIXTURE_PR-"* 2>/dev/null || true
    out=$(timeout "${EDDA_TEST_TIMEOUT_SECONDS:-60}" sh "$root/scripts/review-pr.sh" "$FIXTURE_PR" 1 --dry-run 2>"$tmp/err") || {
        printf 'review-pr.sh --dry-run exited non-zero; stderr:\n%s\n' "$(cat "$tmp/err")" >&2
        return 1
    }
    brief="$EDDA_FLEET_SCRATCH/review-pr$FIXTURE_PR-r1-brief.md"
}

# Same, for a specific round — and optionally under a HOME whose Claude
# session store decides whether the round opens a session or resumes one.
dry_run_round() { # $1=body  $2=round  $3=prev sha (may be empty)  $4=HOME (may be empty)
    case_number=$((case_number + 1))
    printf '%b' "$1" >"$tmp/body"
    export GH_BODY_FILE="$tmp/body"
    rm -f "$EDDA_FLEET_SCRATCH/review-pr$FIXTURE_PR-"* 2>/dev/null || true
    if [ -n "$3" ]; then
        if ! out=$(HOME="${4:-$HOME}" timeout "${EDDA_TEST_TIMEOUT_SECONDS:-60}" sh "$root/scripts/review-pr.sh" \
                "$FIXTURE_PR" "$2" "$3" --dry-run 2>"$tmp/err"); then rc=1; else rc=0; fi
    else
        if ! out=$(HOME="${4:-$HOME}" timeout "${EDDA_TEST_TIMEOUT_SECONDS:-60}" sh "$root/scripts/review-pr.sh" \
                "$FIXTURE_PR" "$2" --dry-run 2>"$tmp/err"); then rc=1; else rc=0; fi
    fi
    if [ "$rc" -ne 0 ]; then
        printf 'review-pr.sh --dry-run (round %s) exited non-zero; stderr:\n%s\n' \
            "$2" "$(cat "$tmp/err")" >&2
        return 1
    fi
    brief="$EDDA_FLEET_SCRATCH/review-pr$FIXTURE_PR-r$2-brief.md"
    lane="$EDDA_FLEET_SCRATCH/review-pr$FIXTURE_PR-r$2-lane.ps1"
}

field() { printf '%s\n' "$out" | sed -n "s/^$1=//p"; }

# --- D1 (#683): the -File argument the scheduled task will receive ------------
# doneWhen: --dry-run shows a -File path with no /c/ prefix.

: >"$tmp/closing"
export GH_CLOSING_FILE="$tmp/closing"
dry_run 'Issue: #650\n'
lane=$(field lane_file_arg)
if [ -z "$lane" ]; then
    fail "D1: --dry-run printed no lane_file_arg= line, so the -File argument cannot be inspected before launch"
else
    case "$lane" in
        /*) fail "D1: the task's -File argument is a POSIX path pwsh.exe cannot resolve: $lane" ;;
        *:\\*) : ;;
        *) fail "D1: the task's -File argument is not a Windows path: $lane" ;;
    esac
fi

# --- D2 (#683): the doneWhen must survive a body without an "Issue:" line -----

dry_run 'Closes #650.\n\nSome prose about the change.\n'
if [ "$(field issues)" != "650" ]; then
    fail "D2a: a body linking its issue as 'Closes #650' yielded issues=$(field issues)"
fi
if ! grep -q '^### Issue #650 doneWhen' "$brief"; then
    fail "D2a: the brief carries no doneWhen section for the issue the body closes"
fi
if ! grep -q 'the launcher reaches Running from Git Bash' "$brief"; then
    fail "D2a: the brief carries the doneWhen heading but not the issue's doneWhen items"
fi

# The repo's own convention still works.
dry_run 'Issue: #650\n\nSome prose.\n'
if [ "$(field issues)" != "650" ]; then
    fail "D2b: an 'Issue: #650' line yielded issues=$(field issues)"
fi
if ! grep -q '^### Issue #650 doneWhen' "$brief"; then
    fail "D2b: the brief carries no doneWhen section for the 'Issue:' line"
fi

# An issue linked only through GitHub's own sidebar linkage (API), not the body.
printf '650\n' >"$tmp/closing"
dry_run 'A body that names no issue at all.\n'
if [ "$(field issues)" != "650" ]; then
    fail "D2c: an issue linked only via the API yielded issues=$(field issues)"
fi
: >"$tmp/closing"

# A body that references other numbers must not be mined for them: #641 below
# is a related PR, not this PR's acceptance ceiling.
dry_run 'Issue: #650\n\nRelated: #641 and #632.\n'
if [ "$(field issues)" != "650" ]; then
    fail "D2d: bare '#N' prose references leaked into the acceptance ceiling: issues=$(field issues)"
fi

# The empty case must be stated, not silently omitted.
dry_run 'A body with no issue link of any kind.\n'
if [ "$(field issues)" != "none" ]; then
    fail "D2e: expected issues=none, got issues=$(field issues)"
fi
if ! grep -qi 'no acceptance criteria' "$brief"; then
    fail "D2e: the brief omits the doneWhen section AND says nothing about it — the reviewer is told to judge against a ceiling it was never given (#683)"
fi
if ! grep -q '#683' "$tmp/err"; then
    fail "D2e: a brief generated with no acceptance criteria printed no warning on stderr"
fi

# --- D3 (#691): the reviewer must never be told to open a browser manual -------
# The spec reaches the reviewer as the worktree copy (.edda-review-spec.md,
# asserted in D8d), so the spec source is checked alongside the brief.

dry_run 'Issue: #650\n'
if grep -nE 'git [a-z][a-z0-9-]* --help' "$brief" "$EDDA_REVIEW_SPEC"; then
    fail "D3: the brief or the spec instructs 'git <verb> --help', which opens the HTML manual in the operator's browser on Windows (#691)"
fi
if ! grep -q 'git <verb> -h' "$EDDA_REVIEW_SPEC"; then
    fail 'D3: the spec does not instruct git <verb> -h as the probe form'
fi
if ! grep -q '691' "$EDDA_REVIEW_SPEC"; then
    fail 'D3: the spec states the -h rule without the reason, so a future editor may restore --help'
fi

# --- D4 (#697): verdict-label reads the Verdict line, not the last keyword ----

expect_label() { # name expected body
    case_number=$((case_number + 1))
    _name=$1; _expected=$2; _body=$3
    if ! _actual=$(printf '%b' "$_body" | timeout 60 sh "$root/scripts/review-pr.sh" verdict-label); then
        fail "$_name: verdict-label exited non-zero"
        return 0
    fi
    [ "$_actual" = "$_expected" ] || fail "$_name: expected '$_expected', got '$_actual'"
}

# The real PR #655 Round 1 verdict: Changes Requested on the Verdict line, the
# word LGTM later in the prose. Observed to invert the label (#697).
PR655='### Verdict\n'
PR655="$PR655"'Changes Requested, P0=1, P1=1\n\n'
PR655="$PR655"'The release-parity gate itself is well-built and I found no defect in it — '
PR655="$PR655"'correct placement, correct `publish`/`yanked` semantics, fails closed, '
PR655="$PR655"'syntax-clean, and its central claim reproduces against live crates.io. The '
PR655="$PR655"'block is entirely that the branch was narrowed in 511ac88 without narrowing '
PR655="$PR655"'the CHANGELOG bullet or the PR body, leaving a false statement that ships '
PR655="$PR655"'into the next release notes. Delete the second CHANGELOG bullet and trim the '
PR655="$PR655"'PR body to the parity gate; that alone should carry this to LGTM.\n'

expect_label \
    'D4a: PR #655 Round 1 — Changes Requested verdict, LGTM in the prose after it' \
    'review:changes-requested' \
    "## Code Review: Round 1 — PR #655 @ 8afc496\n\n### Findings\n- [P0] a finding\n\n$PR655"

expect_label \
    'D4b: an LGTM Verdict line wins even when earlier prose says Changes Requested' \
    'review:lgtm' \
    '## Code Review: Round 2\n\n### Findings\nRound 1 was Changes Requested; both blockers are fixed.\n\n### Verdict\nLGTM (P0=0, P1=0)\n'

expect_label \
    'D4c: no Verdict section is not an LGTM — emit nothing' \
    '' \
    '## Code Review: Round 1\n\nThe run was interrupted before a verdict. Prose mentioning LGTM.\n'

expect_label \
    'D4d: a Verdict section with no verdict keyword emits nothing' \
    '' \
    '## Code Review: Round 1\n\n### Verdict\n(the reviewer stopped here)\n'

# verdict-label must still exit 0 when it emits nothing, so the caller decides.
printf '%b' 'no verdict at all\n' | timeout 60 sh "$root/scripts/review-pr.sh" verdict-label >/dev/null || \
    fail 'D4e: verdict-label must exit 0 when it emits no label'

# --- D5 (GH-708): the shipped transport is edda dispatch --agent claude -------
# Default model claude-opus-5, a valid UUID v4 session id, and a lane script
# that pins the model explicitly and never calls pi.

dry_run 'Issue: #650\n'
if ! grep -q '(read-only, claude-opus-5, session ' "$brief"; then
    fail "D5a: the default review model in the brief header is not claude-opus-5"
fi
sid=$(sed -n 's/.*session \([0-9a-f-]*\))/\1/p' "$brief" | head -1)
case "$sid" in
    ????????-????-5???-[89ab]???-????????????) : ;;
    *) fail "D5b: the session id is not a valid name-based (v5) UUID — the claude backend rejects anything that is not a UUID at all (GH-694 second half), and GH-708 requires it to be derived from the PR number: $sid" ;;
esac
lane="$EDDA_FLEET_SCRATCH/review-pr$FIXTURE_PR-r1-lane.ps1"
if [ ! -f "$lane" ]; then
    fail "D5c: --dry-run wrote no lane script, so the transport cannot be inspected before launch"
else
    grep -q 'edda dispatch --agent claude' "$lane" \
        || fail 'D5d: the lane does not run the edda dispatch claude transport'
    grep -q -- "--model 'claude-opus-5'" "$lane" \
        || fail 'D5e: the lane does not pin the review model with --model (default-only is not allowed)'
    grep -q -- "--exclude-tools 'Edit,Write,NotebookEdit,mcp__\*'" "$lane" \
        || fail 'D5f: the lane does not structurally deny the write tools'
    grep -q -- "--session-id '$sid'" "$lane" \
        || fail 'D5g: the lane does not pass the brief header session UUID'
    if grep -q 'pi -p' "$lane"; then
        fail 'D5h: the lane still calls pi — pi cannot reach any Anthropic model on this fleet (GH-708)'
    fi
    grep -q 'DISPATCH_EXIT=' "$lane" \
        || fail 'D5i: the lane writes no DISPATCH_EXIT receipt into the .done file'
fi

# --- D6 (GH-708): EDDA_REVIEW_MODEL reaches the lane's --model verbatim --------

EDDA_REVIEW_MODEL=claude-opus-5-20260101
export EDDA_REVIEW_MODEL
dry_run 'Issue: #650\n'
if ! grep -q -- "--model 'claude-opus-5-20260101'" "$lane"; then
    fail 'D6: an explicit EDDA_REVIEW_MODEL did not reach the lane as --model'
fi
unset EDDA_REVIEW_MODEL

# --- D7 (GH-708/#699): the closing-keyword ban is narrowed, not blanket --------
# The rule lives in the spec (reached via the worktree copy, D8d), so the spec
# source is the surface to pin.

dry_run 'Issue: #650\n'
if grep -q 'no closing keywords' "$brief" "$EDDA_REVIEW_SPEC"; then
    fail 'D7a: the spec still blanket-bans closing keywords — pr.closing-keyword=only-when-all-donewhen-delivered narrowed it (#699)'
fi
if ! grep -q 'only-when-all-donewhen-delivered' "$EDDA_REVIEW_SPEC"; then
    fail 'D7b: the spec does not state the narrowed closing-keyword policy (#699)'
fi

# --- D8 (GH-708 round 2): the edda dispatch arm must be the arm that runs ------
# Round 1 P1-1: the brief inlined REVIEW.md verbatim (~33k chars), so the lane's
# `$briefChars -lt 30000` guard was never true and every real review silently
# ran the undocumented claude-stdin fallback. The brief now points the reviewer
# at the spec copy in the worktree (.edda-review-spec.md) instead of inlining
# it; these cases pin that shape. Round 1 P1-2: the fallback arm — the only arm
# when a brief ever does exceed the budget — must carry the recorded
# fleet.review-engine-model read-only shape, not an unrestricted reviewer.

dry_run 'Issue: #650\n'
brief_chars=$(wc -m < "$brief")
spec_chars=$(wc -m < "$EDDA_REVIEW_SPEC")
if [ "$brief_chars" -ge 30000 ]; then
    fail "D8a: the brief is $brief_chars chars — at or over the lane's 30000-char guard, so the edda dispatch arm would be skipped for every real brief"
fi
if [ "$brief_chars" -ge "$spec_chars" ]; then
    fail "D8b: the brief ($brief_chars chars) is not smaller than the spec ($spec_chars chars) — the spec is being inlined again"
fi
if grep -q '# review-spec:classifier-end' "$brief"; then
    fail 'D8c: the brief inlines the spec body (found a spec-only marker) — inlining pushed every real brief over the dispatch budget (round 2 P1-1)'
fi
if ! grep -q '.edda-review-spec.md' "$brief"; then
    fail 'D8d: the brief does not point the reviewer at the worktree spec copy .edda-review-spec.md'
fi
lane="$EDDA_FLEET_SCRATCH/review-pr$FIXTURE_PR-r1-lane.ps1"
grep -Fq -- "--tools 'Read,Grep,Glob,Bash(git *),Bash(gh *),Bash(edda *),Bash(sh *)'" "$lane" \
    || fail 'D8e: the fallback arm lacks the measured restricted capability allowlist (GH-702)'
if grep -q 'bypassPermissions' "$lane"; then
    fail 'D8f: the fallback arm still grants --permission-mode bypassPermissions — the recorded fleet.review-engine-model shape auto-approves only the read allowlist'
fi
grep -q 'TRANSPORT=edda-dispatch' "$lane" \
    || fail 'D8g: the dispatch arm writes no TRANSPORT=edda-dispatch receipt — the verdict header cannot name the transport that actually ran'
grep -q 'TRANSPORT=claude-stdin' "$lane" \
    || fail 'D8h: the fallback arm writes no TRANSPORT=claude-stdin receipt'

# --- D9 (GH-708 scope addition): one resumable reviewer conversation per PR ---
# fleet.reviewer-agent=pi-with-per-pr-resumable-session chose pi for one
# measured property: a per-PR reviewer session that RESUMES, so round 2+ reads
# the delta instead of the whole PR. The claude transport must keep it. The id
# is therefore derived from the PR number (identical every round, never the
# implementer's), round 1 opens the conversation and later rounds continue it —
# and the two backends spell "continue" differently, which is the part a
# string-level change silently gets wrong.

# D9a: derivation is deterministic and pinned. If the name string ever changes,
# every PR's reviewer conversation silently forks; this is the tripwire.
EXPECTED_SID_9999=0dc98ad1-d4ae-5321-b81b-b249644368c6   # SHA-1("edda-review-pr9999"), v5 layout
dry_run_round 'Issue: #650\n' 1 '' ''
if [ "$(field session)" != "$EXPECTED_SID_9999" ]; then
    fail "D9a: the reviewer session id for PR $FIXTURE_PR is $(field session), not the derived $EXPECTED_SID_9999"
fi
if [ "$(field session_mode)" != "new" ]; then
    fail "D9a: a PR with no recorded reviewer conversation must open one (session_mode=new), got $(field session_mode)"
fi
grep -q -- "--session-id '$EXPECTED_SID_9999'" "$lane" \
    || fail 'D9b: round 1 does not open the conversation with --session-id'
if grep -qE '(edda dispatch|claude -p) .*--resume' "$lane"; then
    fail 'D9b: round 1 asks to resume a conversation that does not exist yet — claude exits 1 with "No conversation found"'
fi
grep -q "SESSION=$EXPECTED_SID_9999" "$lane" \
    || fail 'D9c: the lane writes no SESSION= receipt, so the watcher cannot report which conversation ran'
grep -q 'SESSION_MODE=new' "$lane" \
    || fail 'D9c: the lane writes no SESSION_MODE= receipt'
grep -qE "reviewer_session: $EXPECTED_SID_9999.*directly under \`model_observed\`" "$brief" \
    || fail 'D9d: the brief does not specify reviewer_session directly under model_observed'
awk '/- model_observed:/{getline; print}' "$EDDA_REVIEW_SPEC" | grep -q 'reviewer_session:' \
    || fail 'D9d: REVIEW.md §7 does not place reviewer_session directly below model_observed'

# The same id on the next round — that is the whole point of deriving it.
dry_run_round 'Issue: #650\n' 2 'cccccccccccccccccccccccccccccccccccccccc' ''
if [ "$(field session)" != "$EXPECTED_SID_9999" ]; then
    fail "D9e: round 2 uses a different reviewer session ($(field session)) — rounds would not accumulate context"
fi

# D9f: with the conversation on disk, both arms must switch to the resume
# spelling — `edda dispatch` keeps --session-id and ADDS --resume (it requires
# the id), while `claude -p` REPLACES --session-id with --resume. Reusing
# --session-id there is not a resume: claude exits 1, "Session ID <id> is
# already in use".
FAKE_HOME="$tmp/home"
mkdir -p "$FAKE_HOME/.claude/projects/C--some-worktree"
: >"$FAKE_HOME/.claude/projects/C--some-worktree/$EXPECTED_SID_9999.jsonl"
dry_run_round 'Issue: #650\n' 2 'cccccccccccccccccccccccccccccccccccccccc' "$FAKE_HOME"
if [ "$(field session_mode)" != "resume" ]; then
    fail "D9f: a recorded reviewer conversation was not resumed (session_mode=$(field session_mode))"
fi
grep -q -- "edda dispatch .*--session-id '$EXPECTED_SID_9999' --resume" "$lane" \
    || fail 'D9f: the dispatch arm does not continue the recorded conversation (--session-id <id> --resume)'
grep -q -- "claude -p .*--resume '$EXPECTED_SID_9999'" "$lane" \
    || fail 'D9f: the fallback arm does not continue the recorded conversation (--resume <id>)'
if grep -q -- "claude -p .*--session-id" "$lane"; then
    fail 'D9f: the fallback arm still names the session with --session-id while resuming — claude refuses an id that already exists'
fi
grep -q 'SESSION_MODE=resume' "$lane" \
    || fail 'D9f: the resumed round writes no SESSION_MODE=resume receipt'
if ! grep -q 'SAME reviewer session' "$brief"; then
    fail 'D9g: a resumed delta round does not tell the reviewer its earlier rounds are in context'
fi

# D9h: a delta round whose conversation is NOT on disk must say so rather than
# let the reviewer assume a context it does not have.
dry_run_round 'Issue: #650\n' 2 'cccccccccccccccccccccccccccccccccccccccc' ''
if ! grep -q 'carries no prior transcript' "$brief"; then
    fail 'D9h: a delta round with no recorded conversation does not warn that the earlier rounds are missing from context'
fi

# D9i: the per-PR worktree is removed when the round ends and pruned before the
# next one — 14 stale wt-review-pr* trees were the reason (GH-708 comment).
grep -q 'function Remove-ReviewWorktree' "$lane" \
    || fail 'D9i: the lane defines no worktree removal'
if [ "$(grep -c 'Remove-ReviewWorktree$' "$lane")" -lt 2 ]; then
    fail 'D9i: the lane does not remove the worktree on both arms'
fi
# Anchored at the start of a line so the assertion cannot be satisfied by the
# comment block above the call (round 1 P2: the loose grep matched prose).
grep -qE '^[[:space:]]*git -C "\$ROOT" worktree prune' "$root/scripts/review-pr.sh" \
    || fail 'D9i: review-pr.sh does not prune stale worktree registrations before adding one at the same per-PR path'

# D9j (round 1 P1): the id the watcher cross-checks must be the backend's OWN
# report, not an echo of the one we launched with — a comparison of a value
# with itself can never fire. Both arms therefore record the launched id AND
# the observed one, on separate lines from separate sources.
grep -q -- 'Session observed: ' "$lane" \
    || fail 'D9j: the fallback arm records no `Session observed:` line, so the watcher has nothing to cross-check the launched id against'
grep -q -- "Session: $EXPECTED_SID_9999" "$lane" \
    || fail 'D9j: the fallback arm no longer records the launched session id alongside the observed one'

# D10 (GH-733): the brief embeds the write-end swallow lens — the wiring-scan
# section and the P1 statement the reviewer needs to act on it. Both asserted
# on the generated brief so the lens cannot silently disappear from the brief.
grep -q 'Wiring and write-end swallow scan' "$brief" \
    || fail 'D10: the brief does not embed the Wiring and write-end swallow scan section (GH-733)'
grep -q 'write-end swallow.*is \*\*P1\*\*' "$brief" \
    || fail 'D10: the brief does not state that a write-end swallow on coordination/ledger/heartbeat/session-ledger/L3-store/digest paths is **P1** (GH-733)'
grep -q '(wiring scan unavailable)' "$brief" \
    || fail 'D10: the brief does not surface (wiring scan unavailable) when revisions cannot be resolved (GH-733)'

# --- offline guarantee: the real fleet scratch carries none of our output -----
# Only our own fixture PR is asserted, not the whole listing: a live watcher or
# another lane may legitimately be writing its own review-pr<N>-* files there
# while this runs, and that is not this test escaping its temp dir.
ls -A "$REAL_SCRATCH" >"$listing_after" 2>/dev/null || : >"$listing_after"
if grep -q "review-pr$FIXTURE_PR-" "$listing_after"; then
    printf 'offline guarantee violated: %s gained review-pr%s-* files:\n' \
        "$REAL_SCRATCH" "$FIXTURE_PR" >&2
    grep "review-pr$FIXTURE_PR-" "$listing_after" >&2 || true
    failures=$((failures + 1))
fi
if ! diff -q "$listing_before" "$listing_after" >/dev/null 2>&1; then
    printf 'note: %s changed during the run (other fleet lanes write there):\n' "$REAL_SCRATCH" >&2
    diff "$listing_before" "$listing_after" >&2 || true
fi

if [ "$failures" != "0" ]; then
    printf 'review-pr fixtures: %s of %s cases failed\n' "$failures" "$case_number" >&2
    exit 1
fi
printf 'review-pr fixtures passed (%s cases)\n' "$case_number"
