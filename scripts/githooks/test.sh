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

echo
if [ "$failed" -eq 0 ]; then
    echo "ALL SCENARIOS PASSED"
else
    echo "SOME SCENARIOS FAILED"
fi
exit "$failed"
