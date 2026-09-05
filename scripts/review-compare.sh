#!/bin/sh
# review-compare.sh — diff the SHADOW review round against the authoritative
# review round on the same SHA (issue #887; operator ruling recorded as
# decision `review.gh880-shadow` = `glm-shadow-round-opus-makeup-when-quota`).
#
# usage: review-compare.sh <pr> <sha>
#   <pr>   PR number
#   <sha>  the reviewed full 40-hex SHA both rounds are pinned to
#
# Environment:
#   EDDA_REPO              owner/repo            (default fagemx/edda)
#   EDDA_COMPARE_FIXTURE   read the comment stream from this file instead of
#                          GitHub (offline tests). The file holds the exact
#                          stream `gh pr view <pr> --json comments --jq
#                          '.comments[] | "<<<COMMENT>>>", .body'` prints:
#                          one <<<COMMENT>>> line before each raw comment
#                          body, oldest comment first.
#
# This script READS GitHub comments only. It never posts, never labels, and
# never writes the ledger; its only output is stdout — the compare table and
# one `for-ledger` line that the controller records as a
# `fleet.review-calibration` row (REVIEW.md §8).
#
# Round selection (REVIEW.md §7/§8): among the §7 comments whose heading is
# `## Code Review: Round <N> — PR #<pr> @ <sha>` — pinned to BOTH the given
# PR number and the given SHA — the SHADOW round is the last one marked
# ` (SHADOW)` in the heading, and the authoritative round is the last one
# without that mark. The `- shadow: true` header field is documentation that
# accompanies the suffix; it never marks a round on its own. "Last" is
# last in the comment stream, which GitHub returns oldest-first.
#
# Findings are the `- [P0|P1|P2] ...` lines of the `### Findings` section.
# Two findings match by rule id (`[U3]`, `[D2]`, ... — #883) when both carry
# one, else by the first `file:line` token in the line, else by the finding
# text itself. The first occurrence of a key wins within one round.
# Classification:
#   missed          in the authoritative round only (counted by severity)
#   unconfirmed     in the SHADOW round only
#   matched         in both rounds, same severity
#   severity drift  in both rounds, different severity
#
# model_observed is read from each round's `- model_observed:` header field;
# a round that reported none is printed as `unverified`, never invented.
#
# Exit codes: 0 = compared, or pending authoritative round (printed, never a
# silent zero — no counts and no for-ledger line in the pending case);
# 2 = usage error, unreadable fixture, or no SHADOW round pinned to that SHA;
# 3 = the GitHub comments fetch failed.
set -u

REPO=${EDDA_REPO:-fagemx/edda}

usage() { echo "usage: review-compare.sh <pr> <sha>" >&2; }

is_full_sha() {
  printf '%s\n' "${1:-}" | grep -Eq '^[0-9a-f]{40}$'
}

[ $# -eq 2 ] || { usage; exit 2; }
PR=$1
SHA=$2
case "$PR" in
  ''|*[!0-9]*) echo "review-compare.sh: <pr> must be a number, got '$PR'" >&2; exit 2 ;;
esac
is_full_sha "$SHA" || {
  echo "review-compare.sh: $SHA is not a full lowercase 40-hex SHA" >&2
  exit 2
}

if [ -n "${EDDA_COMPARE_FIXTURE:-}" ]; then
  [ -r "$EDDA_COMPARE_FIXTURE" ] || {
    echo "review-compare.sh: fixture $EDDA_COMPARE_FIXTURE is not readable" >&2
    exit 2
  }
  stream=$(cat "$EDDA_COMPARE_FIXTURE")
else
  stream=$(gh pr view "$PR" --repo "$REPO" --json comments \
    --jq '.comments[] | "<<<COMMENT>>>", .body' 2>&1)
  rc=$?
  if [ "$rc" -ne 0 ]; then
    echo "review-compare.sh: gh comments fetch failed (exit $rc): $stream" >&2
    exit 3
  fi
fi

# The pin regex is built from two validated values: <pr> is digits only and
# <sha> is exactly 40 lowercase hex characters (REVIEW.md R5), so neither can
# carry regex metacharacters and both match literally (same reasoning as
# pr-review-watch.sh verdict_body_lines).
printf '%s\n' "$stream" | awk -v pr="$PR" -v sha="$SHA" '
  BEGIN { pinre = "^## Code Review: Round [0-9]+( \\(SHADOW\\))? — PR #" pr " @ " sha "$" }

  function trim(s) {
    gsub(/^[[:space:]]+|[[:space:]]+$/, "", s)
    return s
  }

  function flush_comment() {
    if (!inb) return
    inb = 0
    ncom++
    parse_comment(ncom, buf)
    buf = ""
  }

  function store_finding(c, line,   sev, key, disp, rest, ei, dup) {
    sev = "P?"
    if (match(line, /\[P[0-2]\]/)) sev = substr(line, RSTART + 1, RLENGTH - 2)
    key = ""
    if (match(line, /\[(U|D|S|C|R)[0-9]+(\.[0-9]+)?\]/)) {
      key = substr(line, RSTART + 1, RLENGTH - 2)
    } else if (match(line, "[A-Za-z0-9_./~-]+:[0-9]+")) {
      key = substr(line, RSTART, RLENGTH)
    }
    if (key == "") {
      # No rule id and no file:line: fall back to the finding text itself.
      rest = line
      sub(/^- \[P[0-2]\][[:space:]]*/, "", rest)
      ei = index(rest, " — evidence:")
      if (ei > 0) rest = substr(rest, 1, ei - 1)
      gsub(/[[:space:]]+/, " ", rest)
      rest = trim(rest)
      if (rest == "") rest = trim(line)
      key = rest
      disp = (length(key) > 48) ? substr(key, 1, 45) "..." : key
    } else {
      disp = key
    }
    dup = c SUBSEP key
    if (dup in seenkey) return        # first occurrence wins within a round
    seenkey[dup] = 1
    fcnt[c]++
    fsev[c, fcnt[c]] = sev
    fkey[c, fcnt[c]] = key
    fdisp[c, fcnt[c]] = disp
  }

  function parse_comment(c, body,   i, n, L, line, pinned, inhdr, inf,
                         issh, rline, model) {
    n = split(body, L, "\n")
    pinned = 0; inhdr = 1; inf = 0; issh = 0; rline = ""; model = ""
    for (i = 1; i <= n; i++) {
      line = L[i]
      if (!pinned && line ~ pinre) {
        pinned = 1
        rline = line
        if (index(line, "(SHADOW)") > 0) issh = 1
        continue
      }
      if (line ~ /^#{3}[[:space:]]/) {
        inhdr = 0
        inf = (line ~ /^#{3}[[:space:]]*Findings([[:space:]]|$)/) ? 1 : 0
        continue
      }
      if (inhdr && line ~ /^- model_observed:/) {
        model = trim(substr(line, length("- model_observed:") + 1))
        continue
      }
      if (inf && line ~ /^- \[P[0-2]\]/) store_finding(c, line)
    }
    if (!pinned) return
    pinnedc[c] = 1
    isshadow[c] = issh
    modelobs[c] = model
    match(rline, /Round [0-9]+/)
    roundnum[c] = substr(rline, RSTART + 6, RLENGTH - 6)
  }

  { sub(/\r$/, "") }
  /^<<<COMMENT>>>$/ { flush_comment(); inb = 1; buf = ""; next }
  { if (!inb) { inb = 1; buf = "" }
    buf = buf $0 "\n" }
  END {
    flush_comment()
    sc = 0; ac = 0
    for (c = 1; c <= ncom; c++) {
      if (!pinnedc[c]) continue
      if (isshadow[c]) sc = c; else ac = c   # last wins: stream is oldest-first
    }
    if (sc == 0) {
      print "review-compare.sh: no SHADOW round pinned to PR " pr " @ " sha > "/dev/stderr"
      exit 2
    }
    sm = (modelobs[sc] != "") ? modelobs[sc] : "unverified"
    slabel = "Round " roundnum[sc] " (SHADOW)"
    if (ac == 0) {
      printf "shadow-review compare — PR %s @ %s\n", pr, sha
      printf "shadow round: %s (model_observed: %s)\n", slabel, sm
      print "pending authoritative round — no non-SHADOW §7 comment on this PR is pinned to this SHA"
      if (fcnt[sc] + 0 == 0) {
        print "(no findings in the SHADOW round)"
      } else {
        print ""
        print "| finding | shadow |"
        print "|---|---|"
        for (i = 1; i <= fcnt[sc]; i++)
          printf "| %s | %s |\n", fdisp[sc, i], fsev[sc, i]
      }
      exit 0
    }
    am = (modelobs[ac] != "") ? modelobs[ac] : "unverified"
    for (i = 1; i <= fcnt[sc]; i++) shmap[fkey[sc, i]] = fsev[sc, i]
    for (i = 1; i <= fcnt[ac]; i++) authmap[fkey[ac, i]] = fsev[ac, i]
    printf "shadow-review compare — PR %s @ %s\n", pr, sha
    printf "shadow round: %s (model_observed: %s)\n", slabel, sm
    printf "authoritative round: Round %s (model_observed: %s)\n", roundnum[ac], am
    print ""
    print "| class | finding | shadow | authoritative |"
    print "|---|---|---|---|"
    mp0 = 0; mp1 = 0; unc = 0; mat = 0; drf = 0; any = 0
    for (i = 1; i <= fcnt[ac]; i++) {   # missed: authoritative findings the shadow round lacks
      k = fkey[ac, i]; sev = fsev[ac, i]
      if (k in shmap) continue
      any = 1
      if (sev == "P0") mp0++; else if (sev == "P1") mp1++
      printf "| missed | %s | — | %s |\n", fdisp[ac, i], sev
    }
    for (i = 1; i <= fcnt[sc]; i++) {   # unconfirmed: shadow-only findings
      k = fkey[sc, i]
      if (k in authmap) continue         # already matched or drifted below
      any = 1; unc++
      printf "| unconfirmed | %s | %s | — |\n", fdisp[sc, i], fsev[sc, i]
    }
    for (i = 1; i <= fcnt[ac]; i++) {   # matched and severity drift
      k = fkey[ac, i]; sev = fsev[ac, i]; any = 1
      if (!(k in shmap)) continue
      if (shmap[k] == sev) {
        mat++
        printf "| matched | %s | %s | %s |\n", fdisp[ac, i], shmap[k], sev
      } else {
        drf++
        printf "| severity drift | %s | %s | %s |\n", fdisp[ac, i], shmap[k], sev
      }
    }
    if (!any) print "(no findings on either round)"
    printf "\ncounts: missed_P0=%d missed_P1=%d unconfirmed=%d matched=%d drift=%d\n", mp0, mp1, unc, mat, drf
    printf "for-ledger: shadow=%s authoritative=%s missed_P0=%d missed_P1=%d unconfirmed=%d matched=%d drift=%d\n", sm, am, mp0, mp1, unc, mat, drf
    exit 0
  }
'
exit $?
