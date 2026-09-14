#!/bin/sh
# scripts/activation/activate-revision.sh — Job C: one normal activation route.
#
# Merged source, the installed shipping binary, the installed Pi package and the
# agent-manager service carrier each had their own install primitive and could carry
# different revisions. This driver sequences the *existing* primitives in order and
# fails closed on drift or a partial activation. It adds no installer, state store,
# scheduler or daemon: every step is a product command that is also valid alone.
#
# Safety:
#   - Mutates nothing with --dry-run.
#   - Refuses a dirty tracked tree or a HEAD that is not the requested revision.
#   - Never deletes old Pi runtime releases; never kills a PID (the manager step uses
#     the manager's own stop/recover primitives).
#   - Read-only receipt at the end gates success: exit 2 unless coherence is coherent.
#
# usage:
#   sh scripts/activation/activate-revision.sh [--repo PATH] [--revision 40-hex]
#       [--edda-bin PATH] [--edda-from PATH] [--manager-root PATH] [--registry-root PATH]
#       [--no-edda] [--no-pi] [--no-manager] [--dry-run] [--json]
set -eu

usage() {
  cat <<'EOF'
Activate one merged revision across the Edda delivery carriers (Job C)
  --repo PATH          Edda checkout (default: current git toplevel)
  --revision SHA       exact 40-hex revision to activate (default: HEAD)
  --edda-bin PATH      installed shipping binary to verify (default: edda)
  --edda-from PATH     prebuilt binary to copy-install instead of cargo install
  --manager-root PATH  agent-manager service root (default: ~/.edda-agent-manager)
  --registry-root PATH Pi private registry root (default: the installed client default)
  --no-edda            skip the shipping-binary step
  --no-pi              skip the Pi package step
  --no-manager         skip the agent-manager step
  --offline            do not fetch; cannot verify freshness (requires --allow-stale)
  --allow-stale        activate a deliberately older revision than origin/main
  --allow-downgrade    with --allow-stale, allow overwriting newer installed content
  --dry-run            print the plan and mutate nothing; the freshness check uses the
                       existing remote ref without fetching
  --json               accepted; the final receipt is always printed as JSON
  -h, --help           this message
Exit codes: 0 activated and coherent, 1 a step failed, 2 usage/refusal/non-coherent.

By default the route refuses to activate a checkout that is not current `origin/main`,
and under --allow-stale it also refuses to overwrite newer installed content unless
--allow-downgrade is given. This route installs from the checkout, so activating a stale
checkout would silently downgrade installed Pi content or the manager release (issue #1217).
EOF
}

die() { printf 'activate-revision: %s\n' "$*" >&2; exit 2; }

repo=""; revision=""; edda_bin="edda"; edda_from=""; manager_root=""; registry_root=""
dry_run=0; json=0; do_edda=1; do_pi=1; do_manager=1
offline=0; allow_stale=0; allow_downgrade=0

while [ $# -gt 0 ]; do
  case "$1" in
    --repo) [ $# -ge 2 ] || die "missing value for --repo"; repo=$2; shift 2 ;;
    --revision) [ $# -ge 2 ] || die "missing value for --revision"; revision=$2; shift 2 ;;
    --edda-bin) [ $# -ge 2 ] || die "missing value for --edda-bin"; edda_bin=$2; shift 2 ;;
    --edda-from) [ $# -ge 2 ] || die "missing value for --edda-from"; edda_from=$2; shift 2 ;;
    --manager-root) [ $# -ge 2 ] || die "missing value for --manager-root"; manager_root=$2; shift 2 ;;
    --registry-root) [ $# -ge 2 ] || die "missing value for --registry-root"; registry_root=$2; shift 2 ;;
    --no-edda) do_edda=0; shift ;;
    --no-pi) do_pi=0; shift ;;
    --no-manager) do_manager=0; shift ;;
    --offline) offline=1; shift ;;
    --allow-stale) allow_stale=1; shift ;;
    --allow-downgrade) allow_downgrade=1; shift ;;
    --dry-run) dry_run=1; shift ;;
    --json) json=1; shift ;;
    -h|--help) usage; exit 0 ;;
    *) die "unknown option $1" ;;
  esac
done

if [ -z "$repo" ]; then
  repo=$(git rev-parse --show-toplevel 2>/dev/null) || die "--repo is required outside a git checkout"
fi
[ -d "$repo" ] || die "repo $repo does not exist"
repo=$(cd "$repo" && pwd)
head=$(git -C "$repo" rev-parse HEAD) || die "$repo is not a git checkout"
[ -n "$revision" ] || revision=$head
case "$revision" in
  *[!0-9a-f]* | '') die "--revision must be a full 40-hex SHA" ;;
esac
[ "${#revision}" -eq 40 ] || die "--revision must be 40 hex chars"

git -C "$repo" diff --quiet -- 2>/dev/null || die "$repo has unstaged tracked changes; commit or stash first"
git -C "$repo" diff --cached --quiet -- 2>/dev/null || die "$repo has staged changes; commit or stash first"
[ "$head" = "$revision" ] || die "HEAD ($head) is not the target revision ($revision); check out the target first"

# ── Freshness guard (#1217) ────────────────────────────────────────────
# The route installs from the checkout, so activating a checkout that is behind
# origin/main silently downgrades installed content. Refuse unless the operator
# deliberately asks for an older revision. --dry-run never fetches, so its
# "mutates nothing" promise holds; it compares the existing remote ref.
branch=$(git -C "$repo" symbolic-ref --short -q HEAD 2>/dev/null || true)
[ -n "$branch" ] || branch=main
remote_ref="origin/$branch"
if [ "$offline" = 1 ] && [ "$allow_stale" = 0 ]; then
  die "--offline cannot verify that HEAD is current; pass --allow-stale to activate deliberately"
elif [ "$allow_stale" = 1 ]; then
  printf 'activate-revision: WARNING activating a possibly stale revision (%s) because --allow-stale was given\n' "$revision"
else
  if [ "$dry_run" = 1 ]; then
    printf 'plan: git fetch origin %s (dry run does not fetch; comparing the existing %s ref)\n' "$branch" "$remote_ref"
  else
    git -C "$repo" fetch --quiet origin "$branch" 2>/dev/null || die "could not fetch $remote_ref; pass --offline --allow-stale to proceed without the freshness check"
  fi
  origin_rev=$(git -C "$repo" rev-parse --verify --quiet "$remote_ref" 2>/dev/null) || die "cannot resolve $remote_ref; pass --offline --allow-stale to proceed without the freshness check"
  [ "$revision" = "$origin_rev" ] || die "checkout $revision is not current $remote_ref ($origin_rev); run 'git -C $repo fetch && git -C $repo pull --ff-only', or pass --allow-stale to activate a deliberately older revision"
  printf 'activate-revision: freshness ok (HEAD == %s %s)\n' "$remote_ref" "$origin_rev"
fi

receipt="$repo/integrations/pi/activation-receipt.mjs"
[ -f "$receipt" ] || die "missing $receipt (the read-only receipt module)"

receipt_json() {
  set -- --json --repo "$repo" --edda-bin "$edda_bin"
  if [ -n "$registry_root" ]; then set -- "$@" --registry-root "$registry_root"; fi
  if [ -n "$manager_root" ]; then set -- "$@" --manager-root "$manager_root"; fi
  node "$receipt" "$@"
}

run() {
  if [ "$dry_run" = 1 ]; then
    printf 'plan:'
    printf ' %s' "$@"
    printf '\n'
  else
    "$@"
  fi
}

# ── Downgrade guard (#1217) ────────────────────────────────────────────
# Under --allow-stale the checkout may be older than installed state; refuse to
# overwrite newer installed Pi content or a manager configured at a newer
# revision unless the operator explicitly asks for it.
if [ "$allow_stale" = 1 ] && [ "$allow_downgrade" = 0 ]; then
  guard=$(receipt_json)
  installed=$(printf '%s' "$guard" | node -e "let s='';process.stdin.on('data',d=>s+=d).on('end',()=>{const r=JSON.parse(s);process.stdout.write(r.pi.installedReleaseId||'')})")
  repo_id=$(printf '%s' "$guard" | node -e "let s='';process.stdin.on('data',d=>s+=d).on('end',()=>{const r=JSON.parse(s);process.stdout.write(r.pi.repoReleaseId||'')})")
  configured=$(printf '%s' "$guard" | node -e "let s='';process.stdin.on('data',d=>s+=d).on('end',()=>{const r=JSON.parse(s);process.stdout.write((r.manager.configured&&r.manager.configured.headSha)||'')})")
  # Fail closed when the installed identity cannot be observed: "unknown" must not
  # mean "safe to overwrite".
  if [ -n "$repo_id" ] && [ "$installed" != "$repo_id" ]; then
    die "would overwrite installed Pi content ${installed:-unobservable} with this checkout's $repo_id; pass --allow-downgrade to force"
  fi
  if [ -n "$configured" ] && [ "$configured" != "$head" ]; then
    if ! git -C "$repo" cat-file -e "$configured^{commit}" 2>/dev/null; then
      die "the manager is configured at $configured, which is not present in this checkout; cannot prove it is not newer. Pass --allow-downgrade to force"
    fi
    if git -C "$repo" merge-base --is-ancestor "$head" "$configured" 2>/dev/null; then
      die "the manager is configured at $configured, which is newer than this checkout ($head); pass --allow-downgrade to force"
    fi
  fi
fi

printf 'activate-revision: repo=%s revision=%s dryRun=%s\n' "$repo" "$revision" "$dry_run"

# ── Step 1: shipping binary ────────────────────────────────────────────
if [ "$do_edda" = 1 ]; then
  if [ "$dry_run" = 1 ]; then
    if [ -n "$edda_from" ]; then
      printf 'plan: copy-install %s over the resolved %s\n' "$edda_from" "$edda_bin"
    else
      printf 'plan: cargo install --path crates/edda-cli --force (cwd %s)\n' "$repo"
    fi
    printf 'plan: verify %s --version revision matches %s\n' "$edda_bin" "$revision"
  else
    if [ -n "$edda_from" ]; then
      [ -f "$edda_from" ] || die "--edda-from $edda_from does not exist"
      target_bin=$(command -v "$edda_bin" 2>/dev/null) || die "--edda-bin $edda_bin is not on PATH"
      # Copy-install (GH #1133): a release artifact installs by copy, so a running
      # process holding the old file does not fail the install.
      cp "$edda_from" "$target_bin"
    else
      ( cd "$repo" && cargo install --path crates/edda-cli --force )
    fi
    line=$("$edda_bin" --version 2>/dev/null) || die "installed $edda_bin could not report its version"
    rev=$(printf '%s' "$line" | sed -n 's/^[^(]*(\([0-9a-f][0-9a-f]*\).*/\1/p')
    [ -n "$rev" ] || die "unparseable version line: $line"
    case "$revision" in
      "$rev"*) printf 'activate-revision: edda %s matches %s\n' "$rev" "$revision" ;;
      *) die "installed $edda_bin is $line, which does not match $revision" ;;
    esac
  fi
fi

# ── Step 2: Pi package + pinned runtime release ────────────────────────
if [ "$do_pi" = 1 ]; then
  if [ "$dry_run" = 1 ]; then
    printf 'plan: npm pack ./integrations/pi --ignore-scripts (cwd %s)\n' "$repo"
    printf 'plan: npm install --global --ignore-scripts <tarball>\n'
    printf 'plan: edda-pi runtime-install\n'
    printf 'plan: verify installed Pi release id == repo integrations/pi release id\n'
  else
    pack_dir=$(mktemp -d)
    trap 'rm -rf "$pack_dir"' 0 HUP INT TERM
    tgz=$( cd "$repo" && npm pack ./integrations/pi --ignore-scripts --pack-destination "$pack_dir" --json \
      | node -e "let s='';process.stdin.on('data',d=>s+=d).on('end',()=>{const j=JSON.parse(s);process.stdout.write(j[0].filename)})" )
    [ -n "$tgz" ] || die "npm pack produced no tarball name"
    npm install --global --ignore-scripts "$pack_dir/$tgz"
    edda-pi runtime-install >/dev/null
    receipt_json | node -e "let s='';process.stdin.on('data',d=>s+=d).on('end',()=>{const r=JSON.parse(s);const p=r.pi;if(!p.installedReleaseId||p.installedReleaseId!==p.repoReleaseId){console.error('activate-revision: Pi content drift: installed='+p.installedReleaseId+' repo='+p.repoReleaseId);process.exit(1)}process.stderr.write('activate-revision: pi release '+p.installedReleaseId+' matches\n')})"
  fi
fi

# ── Step 3: agent-manager release carrier ──────────────────────────────
if [ "$do_manager" = 1 ]; then
  set -- --repo "$repo" --revision "$revision"
  if [ -n "$manager_root" ]; then set -- "$@" --root "$manager_root"; fi
  if [ "$dry_run" = 1 ]; then set -- "$@" --dry-run; fi
  run node "$repo/scripts/activation/manager-release.mjs" "$@"
fi

# ── Step 4: read-only receipt gate ─────────────────────────────────────
if [ "$dry_run" = 1 ]; then
  printf 'activate-revision: dry run complete; nothing was changed\n'
  exit 0
fi

final=$(receipt_json)
printf '%s\n' "$final"
printf '%s' "$final" | node -e "let s='';process.stdin.on('data',d=>s+=d).on('end',()=>{const r=JSON.parse(s);if(r.coherence.status!=='coherent'){console.error('activate-revision: receipt is '+r.coherence.status+' ('+r.coherence.findings.map(f=>f.code).join(',')+')');process.exit(2)}})"
printf 'activate-revision: coherent at %s\n' "$revision"
