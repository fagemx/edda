#!/bin/sh
# Self-test for scripts/githooks — runs the required scenarios in a throwaway
# repo and prints one PASS/FAIL line per scenario. POSIX sh; needs cargo.
#
# Usage:   sh scripts/githooks/test.sh
# Output is designed to be pasted into the PR body as test evidence.

set -u

hooks_dir=$(cd "$(dirname "$0")" && pwd)
repo=$(mktemp -d)
trap 'rm -rf "$repo"' EXIT

failed=0
report() { # $1 status, $2 desc, $3 detail (optional)
    if [ "$1" = PASS ]; then
        echo "PASS: $2"
    else
        echo "FAIL: $2"
        if [ -n "${3:-}" ]; then
            echo "  detail: $3"
        fi
        failed=1
    fi
}

expect_reject() { # $1 desc, $2 expected-output substring, $3... command
    desc=$1
    want=$2
    shift 2
    out=$("$@" 2>&1) && {
        report FAIL "$desc" "expected rejection, got success: $out"
        return
    }
    case "$out" in
        *"$want"*) report PASS "$desc" ;;
        *) report FAIL "$desc" "rejected, but output missing '$want': $out" ;;
    esac
}

expect_accept() { # $1 desc, $2... command
    desc=$1
    shift
    out=$("$@" 2>&1) || {
        report FAIL "$desc" "$out"
        return
    }
    report PASS "$desc"
}

git init -q "$repo"
cd "$repo" || exit 1
git config user.email hook-test@example.com
git config user.name "hook test"
git config core.hooksPath "$hooks_dir"

echo "temp repo: $repo"
echo "hooks:     $hooks_dir (via core.hooksPath)"
echo

# --- reject 1/4: bad commit message -> commit-msg -------------------------
echo hello > file.txt
git add file.txt
expect_reject "reject 1/4: bad commit message" "commit-msg: rejected" \
    git commit -m "not a conventional message"

# --- accept: wip( lane checkpoint ------------------------------------------
expect_accept "accept: wip( lane checkpoint passes commit-msg" \
    git commit -m "wip(lane): checkpoint"

# --- reject 2/4: cargo fmt failure -> pre-commit ---------------------------
# Temp repo uses the workspace layout so the clippy gate (crates/<c>/ paths)
# is exercised too.
cat > Cargo.toml <<'EOF'
[workspace]
members = ["crates/hooktest"]
resolver = "2"
EOF
mkdir -p crates/hooktest/src
cat > crates/hooktest/Cargo.toml <<'EOF'
[package]
name = "hooktest"
version = "0.1.0"
edition = "2021"
EOF
printf 'fn f()->i32{return 1;}\nfn main(){println!("{}",f());}\n' > crates/hooktest/src/main.rs
git add -A
expect_reject "reject 2/4: cargo fmt failure" "cargo fmt --all --check failed" \
    git commit -m "feat(test): fmt failure"

# --- accept: SKIP_CLIPPY=1 skips clippy, [skip-clippy] tagged --------------
cargo fmt
git add -A
expect_accept "accept: SKIP_CLIPPY=1 skips the clippy gate" \
    env SKIP_CLIPPY=1 git commit -m "feat(test): skip clippy"
git log -1 --format=%B | grep -q '\[skip-clippy\]' \
    && report PASS "SKIP_CLIPPY commit message carries [skip-clippy] tail tag" \
    || report FAIL "SKIP_CLIPPY commit message carries [skip-clippy] tail tag" \
        "tag missing: $(git log -1 --format=%B)"

# --- reject 3/4: cargo clippy failure -> pre-commit ------------------------
git reset -q --soft HEAD~1   # same staged tree, this time without SKIP_CLIPPY
expect_reject "reject 3/4: cargo clippy failure" "cargo clippy -p hooktest failed" \
    git commit -m "feat(test): clippy failure"

# --- accept: fmt+clippy clean change passes, no tag ------------------------
cat > crates/hooktest/src/main.rs <<'EOF'
fn f() -> i32 {
    1
}

fn main() {
    println!("{}", f());
}
EOF
git add -A
expect_accept "accept: fmt+clippy clean change passes all gates" \
    git commit -m "feat(test): passing scenario"
git log -1 --format=%B | grep -q '\[skip-clippy\]' \
    && report FAIL "clean commit carries no [skip-clippy] tag" "unexpected tag" \
    || report PASS "clean commit carries no [skip-clippy] tag"

# --- reject 4/4: staged file > 1 MB -> pre-commit --------------------------
head -c 2097152 /dev/zero > big.bin
git add big.bin
expect_reject "reject 4/4: staged file larger than 1 MB" "the limit is 1 MB" \
    git commit -m "feat(test): oversized file"
git rm -q --cached big.bin
rm -f big.bin

# --- accept: merge commit passes commit-msg --------------------------------
main_branch=$(git symbolic-ref --short HEAD)
git checkout -q -b side
echo side > side.txt
git add side.txt
git commit -q -m "feat(test): side change"
git checkout -q "$main_branch"
expect_accept "accept: merge commit with non-conventional subject" \
    git merge --no-ff side -m "bad merge message"

# --- Round-1 review fixes: one scenario per finding -------------------------
# Each new scenario must FAIL on the pre-fix hook; they are self-contained
# and clean up after themselves (old-hook runs may commit where the fixed
# hook rejects, so every scenario resets to the recorded prev HEAD).

# stub cargo: logs argv to $CARGO_STUB_LOG, exits $CARGO_STUB_EXIT (default 0)
stub_dir="$repo/stubbin"
mkdir -p "$stub_dir"
cat > "$stub_dir/cargo" <<'STUB'
#!/bin/sh
{
    printf 'cargo'
    for a in "$@"; do
        printf ' %s' "$a"
    done
    printf '\n'
} >> "${CARGO_STUB_LOG:?}"
exit "${CARGO_STUB_EXIT:-0}"
STUB
chmod +x "$stub_dir/cargo"
stub_log="$repo/cargo-stub.log"

# --- fix 1: 'Merge ...' subject without an in-progress merge is rejected ----
prev=$(git rev-parse HEAD)
expect_reject "reject: 'Merge ...' subject with no MERGE_HEAD (merge bypass)" \
    "commit-msg: rejected" \
    git commit --allow-empty -m "Merge this is not a merge commit"
git reset -q --hard "$prev"

# --- fix 2: staged >1 MB file with a non-ASCII (Git-quoted) name rejected ---
head -c 1048577 /dev/zero > café.bin
git add café.bin
expect_reject "reject: staged 1 MB+ café.bin survives Git's path quoting" \
    "the limit is 1 MB" \
    git commit -m "feat(test): oversized non-ascii file"
git reset -q --hard "$prev"
rm -f café.bin

# --- fix 3: staged Cargo.custom triggers the fmt gate (Cargo.* glob) --------
echo custom > Cargo.custom
git add Cargo.custom
: > "$stub_log"
expect_reject "reject: staged Cargo.custom runs the fmt gate (Cargo.* glob)" \
    "cargo fmt" \
    env PATH="$stub_dir:$PATH" CARGO_STUB_LOG="$stub_log" CARGO_STUB_EXIT=1 \
    git commit -m "feat(test): cargo custom glob"
grep -q 'fmt --all --check' "$stub_log" \
    && report PASS "staged Cargo.custom invoked cargo fmt" \
    || report FAIL "staged Cargo.custom invoked cargo fmt" \
        "stub log: $(cat "$stub_log" 2>/dev/null)"
git reset -q --hard "$prev"
rm -f Cargo.custom

# --- fix 4: clippy -p uses the package name from Cargo.toml, not the dir ----
mkdir -p crates/namedir/src
cat > crates/namedir/Cargo.toml <<'EOF'
[package]
name = "pkgname-inside"
version = "0.1.0"
edition = "2021"
EOF
printf 'pub fn f() -> i32 {\n    1\n}\n' > crates/namedir/src/lib.rs
git add crates/namedir/Cargo.toml
: > "$stub_log"
prev=$(git rev-parse HEAD)
expect_accept "accept: stub cargo passes the package-name scenario" \
    env PATH="$stub_dir:$PATH" CARGO_STUB_LOG="$stub_log" \
    git commit -m "feat(test): package name from Cargo.toml"
if grep -q 'clippy -p pkgname-inside' "$stub_log" \
    && ! grep -q 'clippy -p namedir' "$stub_log"; then
    report PASS "clippy -p uses the package name from crates/<dir>/Cargo.toml"
else
    report FAIL "clippy -p uses the package name from crates/<dir>/Cargo.toml" \
        "stub log: $(cat "$stub_log" 2>/dev/null)"
fi
git reset -q --hard "$prev"
rm -rf crates/namedir

# --- fix 5: only SKIP_CLIPPY=1 skips; SKIP_CLIPPY=0 runs clippy --------------
prev=$(git rev-parse HEAD)
: > "$stub_log"
echo "# l0 touch" >> crates/hooktest/src/main.rs
git add crates/hooktest/src/main.rs
expect_accept "accept: SKIP_CLIPPY=0 runs the clippy gate (stub passes)" \
    env PATH="$stub_dir:$PATH" CARGO_STUB_LOG="$stub_log" SKIP_CLIPPY=0 \
    git commit -m "feat(test): clippy zero does not skip"
if grep -q 'clippy -p hooktest' "$stub_log" \
    && ! git log -1 --format=%B | grep -qx '\[skip-clippy\]'; then
    report PASS "SKIP_CLIPPY=0 invokes clippy and leaves no [skip-clippy] tag"
else
    report FAIL "SKIP_CLIPPY=0 invokes clippy and leaves no [skip-clippy] tag" \
        "stub log: $(cat "$stub_log" 2>/dev/null); message: $(git log -1 --format=%B)"
fi
git reset -q --hard "$prev"

# --- R3: a newline-named staged path stays ONE record, staged for real ------
# Windows cannot store a real newline in a filename (NTFS forbids control
# characters and `git update-index` refuses them), so git-for-windows encodes
# it as the PUA character U+F00A on disk and keeps those bytes in the index.
# The scenario therefore stages a REAL file via git add — the newline appears
# as U+F00A — and the hook must see ONE NUL-terminated record and reject it
# on size. (On Linux the same hook path handles a real-LF name identically:
# there is no tr / line-splitting anywhere in the listing.)
lfname="$(printf 'large\357\200\212file.bin')"
head -c 1048577 /dev/zero > "$lfname"
git add -- "$lfname"
prev=$(git rev-parse HEAD)
expect_reject "reject: staged 1 MB+ newline-named file as ONE record (real index)" \
    "the limit is 1 MB" \
    git commit -m "feat(test): newline-named oversized file"
git reset -q --hard "$prev"
rm -f "$lfname"

# --- R3: no environment override can hide staged paths from the gates -------
prev=$(git rev-parse HEAD)
head -c 1048577 /dev/zero > hidden.bin
git add hidden.bin
empty_list="$repo/empty-list"
: > "$empty_list"
expect_reject "reject: PRE_COMMIT_STAGED_LIST_Z=/dev/null cannot hide the 1 MB blob" \
    "the limit is 1 MB" \
    env PRE_COMMIT_STAGED_LIST_Z=/dev/null git commit -m "test(scope): oversize"
expect_reject "reject: PRE_COMMIT_STAGED_LIST_Z=<empty file> cannot hide the 1 MB blob" \
    "the limit is 1 MB" \
    env PRE_COMMIT_STAGED_LIST_Z="$empty_list" git commit -m "test(scope): oversize"
if [ "$(git rev-parse HEAD)" = "$prev" ]; then
    report PASS "blob not committed after the override attempts"
else
    report FAIL "blob not committed after the override attempts" "HEAD moved"
fi
git reset -q --hard "$prev"
rm -f hidden.bin

# --- fix 7 (R2): package name from [package], single quotes, other tables ---
# The manifest parser must accept 'literal' as well as "double" names and
# must not leak name = lines from other tables like [dependencies].
mkdir -p crates/litname/src
cat > crates/litname/Cargo.toml <<'EOF'
[package]
name = 'literal-name'
version = "0.1.0"
edition = "2021"

[dependencies]
name = "decoy"
EOF
printf 'pub fn f() -> i32 {\n    1\n}\n' > crates/litname/src/lib.rs
git add crates/litname/Cargo.toml
: > "$stub_log"
prev=$(git rev-parse HEAD)
expect_accept "accept: stub cargo passes the single-quote package-name scenario" \
    env PATH="$stub_dir:$PATH" CARGO_STUB_LOG="$stub_log" \
    git commit -m "feat(test): single-quoted package name"
if grep -q 'clippy -p literal-name' "$stub_log" \
    && ! grep -q 'decoy' "$stub_log" \
    && ! grep -q 'litname' "$stub_log"; then
    report PASS "single-quoted [package] name gives -p literal-name; other tables ignored"
else
    report FAIL "single-quoted [package] name gives -p literal-name; other tables ignored" \
        "stub log: $(cat "$stub_log" 2>/dev/null)"
fi
git reset -q --hard "$prev"
rm -rf crates/litname

# --- GH-692: swallowed hook-path write on an added line is rejected ---------
# Added line under crates/*/src (not a tests file) matching the issue's style
# ('let _ = ledger.append_event(&e);') with no swallow-ok justification must
# block the commit and print the style and line number. Adding
# '// swallow-ok: cleanup only' on the same line lets it through. Stub cargo
# keeps the fmt/clippy gates green so only the ratchet decides.
prev=$(git rev-parse HEAD)
: > "$stub_log"
printf '\nfn gh692_swallow() {
    let _ = ledger.append_event(&e);
}
' >> crates/hooktest/src/main.rs
git add crates/hooktest/src/main.rs
expect_reject "reject: added 'let _ = ledger.append_event(&e);' without swallow-ok (GH-692)" \
    "let _ = ledger.append_event" \
    env PATH="$stub_dir:$PATH" CARGO_STUB_LOG="$stub_log" \
    git commit -m "feat(test): swallowed hook write"
sed -i 's|let _ = ledger.append_event(&e);|let _ = ledger.append_event(&e); // swallow-ok: cleanup only|' crates/hooktest/src/main.rs
git add crates/hooktest/src/main.rs
expect_accept "accept: same write line with '// swallow-ok: cleanup only' passes (GH-692)" \
    env PATH="$stub_dir:$PATH" CARGO_STUB_LOG="$stub_log" \
    git commit -m "feat(test): swallowed hook write justified"
git reset -q --hard "$prev"

echo
if [ "$failed" -eq 0 ]; then
    echo "ALL SCENARIOS PASSED"
else
    echo "SOME SCENARIOS FAILED"
fi
exit "$failed"
