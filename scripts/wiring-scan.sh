#!/bin/sh
# wiring-scan.sh — machine aid for the review-side wiring verdict (issue #594).
#
# Usage: sh scripts/wiring-scan.sh <base> <head>
#
# Section 1 lists `pub` items added by the diff under crates/ (pub
# fn/struct/enum/trait/const/static/type/mod, plus pub fields where
# detectable) with the count of matching lines outside the defining file
# across crates/ at <head> (git grep). In this repo every consumer lives in
# the workspace, so "pub and zero outside references" is a strong dead-
# surface signal. Prints "no new pub surfaces" when none.
#
# Section 2 greps the ADDED lines of the diff for swallow patterns:
#   let _ =  |  .ok();  |  unwrap_or_default()  |  best-effort  |  silently
#
# Human judgement is still required (false positives are expected) — this is
# a reviewer RAN aid, not a CI gate.

set -u

if [ "$#" -ne 2 ]; then
  echo "usage: $0 <base> <head>" >&2
  exit 2
fi
BASE=$1
HEAD=$2

# Fail loudly on unknown revisions — a mistyped review range must never
# false-green with "no new pub surfaces".
for ref in "$BASE" "$HEAD"; do
  if ! git rev-parse --verify --quiet "${ref}^{commit}" >/dev/null 2>&1; then
    echo "error: unknown revision $ref" >&2
    exit 2
  fi
done

echo "== New pub surfaces (${BASE}..${HEAD}) =="

# "<file>\t<kind>\t<name>" for each added pub declaration, deduped.
# Visibility: pub | pub(crate) | pub(super) | pub(in path::to::mod).
# Modifiers: any sequence of async / unsafe / const / extern "C" between
# visibility and the item keyword (e.g. pub(crate) async fn, pub const fn,
# pub unsafe extern "C" fn). Item keywords: fn struct enum trait type const
# static mod union. Fields (pub <name>: Type) handled separately below.
surfaces=$(git diff "$BASE" "$HEAD" --unified=0 -- crates/ | awk '
  /^diff --git / { file = $NF; sub(/^b\//, "", file); next }
  /^\+\+\+/ { next }
  /^\+/ {
    line = substr($0, 2)
    if (match(line, /pub([[:space:]]*\((crate|super|in[[:space:]]+[A-Za-z0-9_:]+)\))?([[:space:]]+(async|unsafe|const|extern([[:space:]]+"[^"]*")))*[[:space:]]+(fn|struct|enum|trait|type|const|static|mod|union)[[:space:]]+[A-Za-z_][A-Za-z0-9_]*/)) {
      decl = substr(line, RSTART, RLENGTH)
      sub(/^pub/, "", decl)
      sub(/^[[:space:]]*\((crate|super|in[[:space:]]+[A-Za-z0-9_:]+)\)/, "", decl)
      while (sub(/^[[:space:]]+(async|unsafe|const|extern([[:space:]]+"[^"]*"))/, "", decl)) { }
      sub(/^[[:space:]]+/, "", decl)
      split(decl, parts, /[[:space:]]+/)
      print file "\t" parts[1] "\t" parts[2]
    } else if (match(line, /pub[[:space:]]+[A-Za-z_][A-Za-z0-9_]*[[:space:]]*:/)) {
      decl = substr(line, RSTART, RLENGTH)
      sub(/^pub[[:space:]]+/, "", decl)
      sub(/[[:space:]]*:$/, "", decl)
      print file "\tfield\t" decl
    }
  }
' | sort -u)

if [ -z "$surfaces" ]; then
  echo "no new pub surfaces"
else
  printf '%s\n' "$surfaces" | while IFS="$(printf '\t')" read -r file kind name; do
    [ -n "$name" ] || continue
    total=$(git grep -w -c "$name" "$HEAD" -- crates/ 2>/dev/null | awk -F: '{ s += $NF } END { print s + 0 }')
    own=$(git grep -w -c "$name" "$HEAD" -- "$file" 2>/dev/null | awk -F: '{ s += $NF } END { print s + 0 }')
    outside=$((total - own))
    echo "$name ($kind)  $file  —  $outside outside references"
  done
fi

echo ""
echo "== Swallow patterns on added lines =="

git diff "$BASE" "$HEAD" --unified=0 | awk '
  /^diff --git / { file = $NF; sub(/^b\//, "", file); next }
  /^\+\+\+/ { next }
  /^\+/ {
    line = substr($0, 2)
    if (line ~ /let _ = |\.ok\(\);|unwrap_or_default\(\)|best-effort|silently/) {
      print file ": " line
      hits++
    }
  }
  END { if (!hits) print "no swallow patterns on added lines" }
'

exit 0
