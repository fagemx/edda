#!/bin/sh
# GH-1093 — the named exit for this carrier. The verdict ladder is the typed
# `edda fleet reclaim` verb (crates/edda-cli/src/cmd_fleet_reclaim.rs); this
# file is the one-line adapter `mechanism.shell-role=one-line-adapter-only`
# allows, so existing callers (`docs/guides/operator-runbook.md` step 9, the
# fleet wave teardown) keep working unchanged.
exec edda fleet reclaim "$@"
