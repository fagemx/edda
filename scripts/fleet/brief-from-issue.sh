#!/bin/sh
# brief-from-issue.sh — render the facts block, nine preamble steps, and
# finish steps of a flash-tier lane brief from a GitHub issue body (GH-885).
# The authored middle is the single marker line <<AUTHORED STEPS>>.
#
# usage:
#   sh scripts/fleet/brief-from-issue.sh <issue> \
#        --lane-name <name> --worktree <path> --branch <branch> \
#        [--task-id <id>] [--build-lane <lane>]
#
# Prints the brief to stdout. Writes no files.
set -eu

usage() {
    echo "usage: $0 <issue> --lane-name <name> --worktree <path> --branch <branch> [--task-id <id>] [--build-lane <lane>]" >&2
}

die() {
    echo "brief-from-issue: $1" >&2
    exit 2
}

repo=${EDDA_REPO:-fagemx/edda}
issue=
lane_name=
worktree=
branch=
task=none
lane=none

while [ $# -gt 0 ]; do
    case "$1" in
        --lane-name)
            [ -n "${2:-}" ] || die "--lane-name requires a value"
            lane_name=$2; shift 2 ;;
        --worktree)
            [ -n "${2:-}" ] || die "--worktree requires a value"
            worktree=$2; shift 2 ;;
        --branch)
            [ -n "${2:-}" ] || die "--branch requires a value"
            branch=$2; shift 2 ;;
        --task-id)
            [ -n "${2:-}" ] || die "--task-id requires a value"
            task=$2; shift 2 ;;
        --build-lane)
            [ -n "${2:-}" ] || die "--build-lane requires a value"
            lane=$2; shift 2 ;;
        -h|--help)
            usage; exit 0 ;;
        --)
            shift; break ;;
        -*)
            die "unknown option $1" ;;
        *)
            if [ -z "$issue" ]; then
                issue=$1; shift
            else
                die "unexpected argument $1"
            fi
            ;;
    esac
done

[ -n "$issue" ] || { usage; exit 2; }
[ -n "$lane_name" ] || die "missing --lane-name"
[ -n "$worktree" ] || die "missing --worktree"
[ -n "$branch" ] || die "missing --branch"
case "$issue" in
    *[!0-9]*|'') die "issue must be a positive integer, got '$issue'" ;;
esac

self_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
root=$(CDPATH= cd -- "$self_dir/../.." && pwd)

json=$(gh issue view "$issue" --repo "$repo" --json body,title,labels) ||
    die "gh issue view $issue failed"

title=$(printf '%s' "$json" | jq -r .title)
body=$(printf '%s' "$json" | jq -r .body)
[ -n "$title" ] && [ "$title" != "null" ] || die "issue $issue has no title"
[ -n "$body" ] && [ "$body" != "null" ] || die "issue $issue has an empty body"

# The title is interpolated verbatim into quoted shell strings inside the
# brief (steps 15 and 21), so double quotes, backticks, and dollar signs are
# stripped: a backtick or $(...) surviving into step 15 would be expanded by
# the lane's shell when it runs that step.
title_safe=$(printf '%s' "$title" | tr -d '"`$')

section=$(printf '%s\n' "$body" | tr -d '\r' | awk '
    /^##[ \t]+Predicted surface[ \t]*$/ { p=1; next }
    /^##[ \t]/ { if (p) exit }
    p { print }
')
[ -n "$section" ] || die "missing or empty ## Predicted surface section"

is_repo_path() {
    case "$1" in
        http://*|https://*) return 1 ;;
        */*) return 0 ;;
        *.sh|*.ps1|*.md|*.rs|*.toml|*.yml|*.yaml|*.json|*.lock) return 0 ;;
    esac
    return 1
}

tmp=$(mktemp "${TMPDIR:-/tmp}/brief-from-issue.XXXXXX")
raw=$(mktemp "${TMPDIR:-/tmp}/brief-from-issue.XXXXXX")
trap 'rm -f "$tmp" "$raw"' 0 HUP INT TERM

# Backtick tokens come in pairs with awk RS='`': odd records are the prose
# between tokens, even records are the tokens themselves. A token whose prose
# prefix ends in no/not/without is a negated mention ("No crate, no `REVIEW.md`."),
# not a listed path, and is dropped here so the scope never grows a path the
# issue explicitly excluded.
printf '%s' "$section" | awk -v RS='`' '
    NR % 2 == 1 { prev = $0; next }
    {
        p = prev
        sub(/[ \t\r\n]+$/, "", p)
        if (p !~ /([Nn]o|[Nn]ot|[Ww]ithout)$/) print
    }
' >"$raw"
: >"$tmp"
while IFS= read -r tok; do
    [ -n "$tok" ] || continue
    is_repo_path "$tok" || continue
    if grep -Fxq -- "$tok" "$tmp" 2>/dev/null; then
        continue
    fi
    printf '%s\n' "$tok" >>"$tmp"
done <"$raw"

[ -s "$tmp" ] || die "missing or empty ## Predicted surface section"

paths_csv=
paths_claim=
paths_space=
npath=0
while IFS= read -r p; do
    [ -n "$p" ] || continue
    npath=$((npath + 1))
    if [ -z "$paths_csv" ]; then
        paths_csv=$p
        paths_space=$p
    else
        paths_csv="$paths_csv, $p"
        paths_space="$paths_space $p"
    fi
    paths_claim="$paths_claim --paths $p"
done <"$tmp"

sha=$(git -C "$root" rev-parse origin/main) || die "git rev-parse origin/main failed"
case "$sha" in
    [0-9a-f]*) ;;
    *) die "origin/main SHA is empty" ;;
esac

host=$(uname -s)
case "$host" in
    MINGW*|MSYS*|CYGWIN*)
        host_fact='Host: MINGW/MSYS. review-pr.sh --dry-run generates -lane.ps1 and no -run.sh.'
        ;;
    *)
        host_fact='Host: Linux. review-pr.sh --dry-run generates -run.sh and no -lane.ps1.'
        ;;
esac

# Porcelain expected lines: one path per line, listed for the git-status finish step.
status_lines=$(printf '%s\n' "$paths_space" | tr ' ' '\n' | sed '/^$/d')

cat <<EOF
role: worker · lane: ${lane} · task id: ${task} · issue: #${issue} ·
base full SHA: ${sha} ·
scope paths: ${paths_csv} ·
entry: none: procedure below · gate owner: not you; review queue ·
out-of-scope: every path and concern not listed in scope paths

Launcher contract: you are pi 0.84.4 in an exclusively owned, clean worktree at ${worktree} on branch ${branch} at the base full SHA, with task ${task} assigned, issue #${issue} claimed, lane name ${lane_name}, and no existing PR for that branch. Your tools are bash({"command": string}) running Git Bash, write({"path": string, "content": string}) and edit({"path": string, "edits": [{"oldText": string, "newText": string}]}). Shell commands go in bash.command; file changes are write/edit tool calls, never shell programs that rewrite files (no sed -i, no cat > file). git, gh and edda are authenticated on PATH. Build lane: ${lane}.

${host_fact}

Failure rule for every numbered step: a nonzero command exit, tool error,
or output-schema mismatch stops execution. Report exactly
STOP step=<number> output=<verbatim unexpected output>. Preserve all files;
do not restore, checkout, reset, clean, retry, review, or merge. The controller
issues the next brief. Success advances to the next numbered step.

1. gh issue view ${issue} --repo ${repo} --json state --jq .state
   output: OPEN.
2. git status --porcelain=v1 --untracked-files=all --branch
   output: exactly ## ${branch}...origin/main, no other lines.
3. git rev-parse HEAD
   output: ${sha}.
4. edda context
   output: context text, exit 0; an off-limits overlap with a scope path is a STOP under the failure rule.
5. edda task list
   output: task-list text, exit 0, including task ${task}.
6. edda task show ${task}
   output: task ${task} and this brief, exit 0.
7. edda claim gh${issue}-worker${paths_claim}
   output: successful claim for gh${issue}-worker and the scope paths, exit 0.
8. gh issue view ${issue} --repo ${repo} --json body --jq .body
   output: the issue body, exit 0. Read doneWhen there; do not copy doneWhen into this brief.
9. git ls-files -- ${paths_space}
   output: the tracked subset of the scope paths, one per line, exit 0. cat each printed path. Untracked scope paths are created in the authored steps; that is not a STOP.

<<AUTHORED STEPS>>

10. git status --porcelain=v1 --untracked-files=all
    output: exactly the scope paths and no others, one porcelain line per path:
${status_lines}
11. git diff --check
    output: empty stdout, exit 0.
12. git add -- ${paths_space}
    output: empty stdout, exit 0.
13. git diff --cached --name-only
    output: exactly the scope paths, one path per line, and no others.
14. git diff --cached --stat
    output: ${npath} path-stat lines and one summary line for exactly ${npath} files.
15. git commit -m "${title_safe}" -m "Issue: #${issue}"
    output: git commit summary, exit 0.
16. git log -1 --format=%B | grep -Fx 'Issue: #${issue}'
    output: Issue: #${issue}.
17. git status --porcelain=v1 --untracked-files=all
    output: empty stdout, exit 0.
18. git rev-parse HEAD
    output: one 40-character hexadecimal SHA; retain as delivery_sha.
19. git push --porcelain -u origin ${branch}
    output: Git porcelain push status, exit 0, no rejected ref. A normal push; no force option is authorized.
20. write({"path":".git/gh${issue}-pr-body.md","content":PR_BODY}) with PR_BODY containing exactly these sections in this order: ## Problem and change; ## Validation with ### RAN and ### READ; then the line Issue: #${issue}. pr.closing-keyword: a closing keyword is allowed only when every doneWhen item of #${issue} is delivered.
    output: Successfully wrote bytes to .git/gh${issue}-pr-body.md.
21. gh pr create --repo ${repo} --base main --head ${branch} --title "${title_safe}" --body-file .git/gh${issue}-pr-body.md
    output: one https://github.com/${repo}/pull/<integer> URL, exit 0. Retain as pr_url.
22. gh pr view ${branch} --repo ${repo} --json headRefOid --jq .headRefOid
    output: delivery_sha from step 18, exactly.
23. edda task done ${task} --receipt "PR <pr_url> @ <delivery_sha>"
    output: task ${task} marked done, exit 0.
24. Report, as the final message, exactly these five lines and nothing else:
    DONE issue=#${issue} task=${task}
    pr=<pr_url>
    sha=<delivery_sha>
    tests=exit 0
    stop=controller issues the next brief
EOF
