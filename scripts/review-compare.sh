#!/bin/sh
# review-compare.sh — diff a SHADOW round against the authoritative round on
# the same SHA (GH-887, review.gh880-shadow). Reads GitHub comments only;
# never posts, labels, or writes the ledger. The output feeds
# fleet.review-calibration rows and the #888 return checklist.
#
# usage:
#   sh scripts/review-compare.sh <pr> <sha>
#
# Output: a missed/unconfirmed/matched/severity-drift table and one
# `for-ledger` line. Exit 0; exit 2 when there is no SHADOW round to compare.
set -eu

usage() {
    echo "usage: $0 <pr> <sha>" >&2
}

die() {
    echo "review-compare: $1" >&2
    exit 2
}

repo=${EDDA_REPO:-fagemx/edda}
pr=${1:-}
sha=${2:-}
[ -n "$pr" ] && [ -n "$sha" ] || { usage; exit 2; }
case "$pr" in *[!0-9]*|'') die "pr must be a positive integer, got '$pr'" ;; esac
case "$sha" in
    *[!0-9a-f]*|'') die "sha must be a 40-hex value" ;;
esac
printf '%s' "$sha" | grep -qE '^[0-9a-f]{40}$' || die "sha must be a full 40-hex SHA, got '${sha}'"

comments=$(gh pr view "$pr" --repo "$repo" --json comments \
    --jq '.comments[] | "<<<COMMENT>>>", .body' 2>&1) ||
    die "gh pr view $pr comments failed"

tmp=$(mktemp "${TMPDIR:-/tmp}/review-compare.XXXXXX")
findings_tmp2=$(mktemp "${TMPDIR:-/tmp}/review-compare.XXXXXX")
trap 'rm -f "$tmp" "$tmp".* "$findings_tmp2" "$findings_tmp2".*' 0 HUP INT TERM

# Extract, per comment pinned to the sha, the verdict line, model_observed,
# and the Findings rows into:
#   $tmp.shadow.findings / $tmp.auth.findings   — "- [Pn] ..." lines
#   $tmp.shadow.meta   / $tmp.auth.meta         — verdict<TAB>model_observed
printf '%s' "$comments" | awk -v sha="$sha" -v tmp="$tmp" '
    function flush(   i, n, pinned, isshadow, inh, section, m) {
      if (!inb) return
      inb = 0
      n = split(buf, L, "\n")
      pinned = 0; isshadow = 0; inh = 0; section = ""
      model = ""; verdict = ""
      for (i = 1; i <= n; i++) {
        # Heading pin without a regex over the em dash: gawk in the C locale
        # counts bytes, and the dash is three of them, so a character-count
        # regex silently misaligns. Split on " @ " and compare the tail.
        if (L[i] ~ /^## Code Review: Round [0-9]+ /) {
          p = index(L[i], " @ ")
          if (p > 0) {
            tail = substr(L[i], p + 3)
            if (tail == sha || tail == sha " (SHADOW)") {
              pinned = 1; continue
            }
          }
        }
        if (pinned && (L[i] == "shadow: true" || L[i] == "- shadow: true")) isshadow = 1
        if (pinned && L[i] ~ /^- model_observed: /) {
          model = L[i]
          sub(/^- model_observed: /, "", model)
        }
        if (pinned && L[i] ~ /^#{1,}[[:space:]]*Verdict/) { inh = 1; section = "verdict"; continue }
        if (pinned && L[i] ~ /^#{1,3}[[:space:]]*Findings/) { section = "findings"; continue }
        if (pinned && L[i] ~ /^#{1,3}[[:space:]]*[A-Za-z]/ && L[i] !~ /Findings/ && L[i] !~ /Verdict/) {
          if (section == "findings") section = ""
        }
        if (pinned && inh && verdict == "" && L[i] ~ /LGTM|Changes Requested/) {
          verdict = L[i]
        }
        if (pinned && section == "findings" && L[i] ~ /^- \[P[0-9]\]/) {
          print L[i] > (isshadow ? tmp ".shadow.findings" : tmp ".auth.findings")
        }
      }
      if (pinned)
        print verdict "\t" model > (isshadow ? tmp ".shadow.meta" : tmp ".auth.meta")
    }
    { sub(/\r$/, "") }
    /^<<<COMMENT>>>$/ { flush(); inb = 1; buf = ""; next }
    { if (!inb) { inb = 1; buf = "" }
      buf = buf $0 "\n" }
    END { flush() }
'

[ -f "$tmp.shadow.findings" ] || die "no SHADOW round pinned to $sha on PR #$pr — nothing to compare"

# Matching key per finding: every file:line and bare-file token in the line;
# a finding with no file token falls back to its first 60 characters. Two
# findings sharing any key are the same finding (#883 will make rule ids the
# primary key; the fallback keeps this usable before that lands).
mv "$tmp.shadow.findings" "$findings_tmp2.shadow"
if [ -f "$tmp.auth.findings" ]; then
    mv "$tmp.auth.findings" "$findings_tmp2.auth"
else
    printf '' >"$findings_tmp2.auth"
fi

awk -v sf="$findings_tmp2.shadow" '
    function keys(line,   s, k) {
      k = ""
      line = tolower(line)
      s = line
      while (match(s, /[a-z0-9_./-]+:[0-9]+/)) {
        k = k "|" substr(s, RSTART, RLENGTH)
        s = substr(s, RSTART + RLENGTH)
      }
      if (k == "") {
        s = line
        sub(/^- \[P[0-9]\] */, "", s)
        k = "|" substr(s, 1, 60)
      }
      return k
    }
    BEGIN {
      sn = 0
      while ((getline line < sf) > 0) { sn++; sk[sn] = keys(line); sl[sn] = line }
      close(sf)
    }
    {
      ak = keys($0)
      hit = 0
      for (i = 1; i <= sn; i++) {
        if (index(ak, sk[i]) > 0 || index(sk[i], ak) > 0) {
          hit = 1
          a = $0; b = sl[i]
          sub(/^[^P]*P/, "", a); sub(/^[^P]*P/, "", b)
          ap = substr(a, 1, 1); bp = substr(b, 1, 1)
          if (ap == bp) print "matched\t" $0 "\t" sl[i]
          else          print "drift\t" $0 "\t" sl[i]
          shadow_used[i] = 1
          break
        }
      }
      if (!hit) print "missed\t" $0
    }
    END {
      for (i = 1; i <= sn; i++)
        if (!shadow_used[i]) print "unconfirmed\t" sl[i]
    }
' "$findings_tmp2.auth" >"$tmp.rows"

shadow_model=$(sed -n 's/^[^\t]*\t//p' "$tmp.shadow.meta" | head -1)
auth_verdict=$(head -1 "$tmp.auth.meta" 2>/dev/null | cut -f1)
auth_model=$(head -1 "$tmp.auth.meta" 2>/dev/null | cut -f2)

echo "## SHADOW vs authoritative — PR #$pr @ $sha"
echo
echo "- shadow verdict/model: $(sed -n 's/^\([^\t]*\)\t/\1 /p' "$tmp.shadow.meta" | head -1)"
if [ ! -s "$tmp.auth.meta" ]; then
    echo "- authoritative: **pending authoritative round** — the rows below are the shadow findings for the record"
    echo
    cat "$findings_tmp2.shadow"
    echo
    echo "for-ledger: shadow=$shadow_model authoritative=pending missed_P0=0 missed_P1=0 unconfirmed=$(grep -c '^unconfirmed' "$tmp.rows" 2>/dev/null || true) matched=0 drift=0"
    exit 0
fi
echo "- authoritative verdict/model: $auth_verdict / ${auth_model:-unknown}"
echo
echo "| kind | authoritative finding | shadow finding |"
echo "|---|---|---|"
grep '^matched' "$tmp.rows" | awk -F'\t' '{ gsub(/\|/, "\\|", $2); gsub(/\|/, "\\|", $3); print "| matched | " $2 " | " $3 " |" }'
grep '^drift' "$tmp.rows" | awk -F'\t' '{ gsub(/\|/, "\\|", $2); gsub(/\|/, "\\|", $3); print "| **severity drift** | " $2 " | " $3 " |" }'
grep '^missed' "$tmp.rows" | awk -F'\t' '{ gsub(/\|/, "\\|", $2); print "| **missed** | " $2 " | — |" }'
grep '^unconfirmed' "$tmp.rows" | awk -F'\t' '{ gsub(/\|/, "\\|", $2); print "| **unconfirmed** | — | " $2 " |" }'
echo
mp0=$(grep -c '^missed.*\[P0\]' "$tmp.rows" || true)
mp1=$(grep -c '^missed.*\[P1\]' "$tmp.rows" || true)
unc=$(grep -c '^unconfirmed' "$tmp.rows" || true)
mat=$(grep -c '^matched' "$tmp.rows" || true)
dri=$(grep -c '^drift' "$tmp.rows" || true)
shadow_model=$(sed -n 's/^[^\t]*\t//p' "$tmp.shadow.meta" | head -1)
echo "for-ledger: shadow=$shadow_model authoritative=$auth_model missed_P0=$mp0 missed_P1=$mp1 unconfirmed=$unc matched=$mat drift=$dri"
