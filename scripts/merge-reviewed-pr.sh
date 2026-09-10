#!/bin/sh
# merge-reviewed-pr.sh — one-line adapter (GH-1105; mechanism.shell-role=
# one-line-adapter-only). The merge preconditions are the product verb
# `edda review merge` (crates/edda-cli/src/cmd_review/merge.rs): the
# fleet-wide drift gate (GH-993), the trusted-review checks, the union
# rule (GH-769/GH-742), malformed-comment refusal (#917), required
# checks, and — folded from #1100 — a squash subject always derived from
# the validated PR title and a merge body file. Usage is unchanged:
# merge-reviewed-pr.sh PR [--merge]; --check is the default and --merge
# requires operator authority. The verb is read-only without --merge and
# no longer performs delivery writes — `edda review deliver` owns those.
exec "${EDDA_BIN:-edda}" review merge --pr "${1:?usage: merge-reviewed-pr.sh PR [--merge]}" ${2:+"$2"}
