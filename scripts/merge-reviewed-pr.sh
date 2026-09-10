#!/bin/sh
# merge-reviewed-pr.sh — one-line adapter (GH-1105; mechanism.shell-role=
# one-line-adapter-only). The merge preconditions are the product verb
# `edda review merge` (crates/edda-cli/src/cmd_review/merge.rs): the
# fleet-wide drift gate (GH-993), the trusted-review checks, the union
# rule (GH-769/GH-742), malformed-comment refusal (#917), required
# checks, and — folded from #1100 — a squash subject always derived from
# the validated PR title and carrying its ` (#N)` back-reference, plus a
# merge body file. The verb is read-only without --merge and no longer
# performs delivery writes — `edda review deliver` owns those.
#
# Usage is unchanged: merge-reviewed-pr.sh PR [--check|--merge]
# [--body-file <path>]; --check is the default and --merge requires
# operator authority. "Unchanged" is a claim this file has to keep, so
# EVERY argument after the PR number is forwarded ("$@"), not just the
# first one: --body-file takes an operand, so an adapter that passed a
# single argument dropped #1100's doneWhen 3 — and anything after it —
# on the floor while still promising the old calls worked.
if [ "${1:-}" = --help ]; then
  echo 'usage: merge-reviewed-pr.sh PR [--check|--merge] [--body-file <path>] (merge requires operator authority)'
  echo 'Everything after PR is forwarded to `edda review merge --pr PR` unchanged:'
  exec "${EDDA_BIN:-edda}" review merge --help
fi
pr=${1:?usage: merge-reviewed-pr.sh PR [--check|--merge] [--body-file <path>]}
shift
exec "${EDDA_BIN:-edda}" review merge --pr "$pr" "$@"
