#!/usr/bin/env bash
set -euo pipefail
root=$(git rev-parse --show-toplevel)
lint="$root/scripts/lint-doc-citations.sh"
hook="$root/scripts/githooks/pre-commit"
tmp=$(mktemp -d)
trap 'rm -rf -- "$tmp"' EXIT
git init -q "$tmp/repo"
cd "$tmp/repo"
git config user.name citation-test
git config user.email citation-test@example.invalid
git config core.autocrlf false
mkdir -p crates/demo/src crates/demo/tests
printf '%s\n' 'header' 'pub fn alpha() {}' 'tail' > crates/demo/src/lib.rs
printf '%s\n' 'test fixture' > crates/demo/tests/contract.rs
printf '%s\n' '`crates/demo/src/lib.rs:2-3#pub fn alpha()`' > README.md
git add .
git -c core.hooksPath=/dev/null commit -qm fixture
checks=0
pass() {
    bash "$lint" "${1:---tree}" > "$tmp/output" 2>&1 || {
        cat "$tmp/output" >&2; echo "expected citation success" >&2; exit 1;
    }
    checks=$((checks+1))
}
fail() {
    local expected=$1 mode=${2:---tree}
    if bash "$lint" "$mode" > "$tmp/output" 2>&1; then
        echo "expected citation rejection: $expected" >&2; exit 1
    fi
    if ! awk -v expected="$expected" 'index($0,expected) { found=1 } END {exit !found}' "$tmp/output"; then
        cat "$tmp/output" >&2; echo "missing diagnostic: $expected" >&2; exit 1
    fi
    checks=$((checks+1))
}
pass
pass --staged
# Literal substring, not regex; metacharacters must match literally.
printf '%s\n' '`crates/demo/src/lib.rs:2#pub fn alpha.*`' > README.md
fail 'literal anchor mismatch'
printf '%s\n' '`crates/demo/src/lib.rs:3-8`' > README.md
fail 'out-of-bounds citation'
printf '%s\n' '`crates/demo/src/lib.rs:3-2`' > README.md
fail 'out-of-bounds citation'
printf '%s\n' '`crates/demo/src/lib.rs:0`' > README.md
fail 'out-of-bounds citation'
printf '%s\n' '`crates/demo/src/missing.rs:1`' > README.md
fail 'missing citation target'
printf '%s\n' '`crates/demo/src/lib.rs#pub fn alpha()`' > README.md
pass
printf '%s\n' '`crates/demo/src/lib.rs#pub fn gone()`' > README.md
fail 'literal anchor mismatch'
# Inner and outer Rust doc comments, including plain root-relative paths.
git checkout -- README.md
printf '%s\n' '//! `tests/contract.rs`' '/// `crates/demo/src/lib.rs:99`' > crates/demo/src/docs.rs
git add crates/demo/src/docs.rs
fail 'tests/contract.rs'
printf '%s\n' '//! `crates/demo/tests/contract.rs`' '/// `crates/demo/src/lib.rs:2#pub fn alpha()`' > crates/demo/src/docs.rs
pass
git add .
git -c core.hooksPath=/dev/null commit -qm docs
# Source-only shift: unchanged staged Markdown must still be checked.
printf '%s\n' 'inserted one' 'inserted two' 'header' 'pub fn alpha() {}' 'tail' > crates/demo/src/lib.rs
git add crates/demo/src/lib.rs
fail 'README.md:1: literal anchor mismatch' --staged
# Working-tree source repair must not mask a broken staged source.
git show HEAD:crates/demo/src/lib.rs > crates/demo/src/lib.rs
pass
fail 'README.md:1: literal anchor mismatch' --staged
# Inverse: a working-tree-only break cannot poison a good index.
git reset -q HEAD -- crates/demo/src/lib.rs
printf '%s\n' 'inserted one' 'inserted two' 'header' 'pub fn alpha() {}' 'tail' > crates/demo/src/lib.rs
pass --staged
fail 'literal anchor mismatch'
git checkout -- crates/demo/src/lib.rs
# Working-tree Markdown correction cannot conceal a stale staged citation.
printf '%s\n' '`crates/demo/src/lib.rs:1#pub fn alpha()`' > README.md
git add README.md
git show HEAD:README.md > README.md
pass
fail 'README.md:1: literal anchor mismatch' --staged
git reset -q HEAD -- README.md
# Source deletion must fail through the actual hook, before its ACMR return.
git rm -q crates/demo/src/lib.rs
if bash "$hook" > "$tmp/output" 2>&1; then
    echo 'source deletion escaped the pre-commit citation gate' >&2; exit 1
fi
awk '/missing citation target/ {found=1} END {exit !found}' "$tmp/output"
checks=$((checks+1))
git reset -q HEAD -- crates/demo/src/lib.rs
git checkout -- crates/demo/src/lib.rs
# Citing file names with spaces/newlines are not split by the scanner.
printf '%s\n' '`crates/demo/src/lib.rs:90`' > $'space and\nnewline.md'
git add .
fail 'out-of-bounds citation'
printf '%s\n' '```text' '`crates/demo/src/missing.rs:99`' '```' > $'space and\nnewline.md'
pass

if [ "${1:-}" = --history ]; then
    # Use genuine historical documents and sources. Only add the anchor
    # protocol to the original numeric range, without changing its endpoints.
    historical() {
        local rev=$1 path=$2 pattern=$3 anchor=$4 expected=$5
        mkdir -p "$tmp/history/$rev/$(dirname "$path")"
        cd "$tmp/history/$rev"
        git init -q
        git -C "$root" show "$rev:COMPATIBILITY.md" > "$tmp/historical-doc"
        git -C "$root" show "$rev:$path" > "$path"
        awk -v pattern="$pattern" -v anchor="$anchor" -v path="$path" '
            index($0, pattern) {
                line=$0
                while (match(line, /`[^`]+`/)) {
                    token=substr(line,RSTART+1,RLENGTH-2)
                    if (index(token,pattern)) {
                        sub(/^[^:]+:/, "", token)
                        for (i=1; i<NR; i++) print ""
                        print "`" path ":" token "#" anchor "`"
                        found=1; exit
                    }
                    line=substr(line,RSTART+RLENGTH)
                }
            }
            END {exit !found}
        ' "$tmp/historical-doc" > COMPATIBILITY.md
        git add .
        if [ "$expected" = pass ]; then pass; else fail 'literal anchor mismatch'; fi
        printf 'history %s %s: %s\n' "$rev" "$pattern" "$expected"
    }
    historical 93eceee crates/edda-cli/src/main.rs 'main.rs:483-489' 'With --json, exactly one object is printed to stdout:' fail
    historical 0a94ecd crates/edda-cli/src/main.rs 'main.rs:488-494' 'With --json, exactly one object is printed to stdout:' pass
    historical fb6ab1b crates/edda-cli/src/cmd_dispatch.rs 'cmd_dispatch.rs:259-268' 'pub fn to_json(&self) -> String {' fail
    historical fb6ab1b crates/edda-cli/src/cmd_dispatch.rs 'cmd_dispatch.rs:114-122' 'pub enum Outcome {' fail
    historical fb6ab1b crates/edda-cli/src/cmd_dispatch.rs 'cmd_dispatch.rs:127-134' 'pub fn exit_code_for(outcome: Outcome) -> i32 {' fail
fi
printf 'doc-citations: %s checks passed\n' "$checks"
