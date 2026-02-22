#!/usr/bin/env bash
# Setup script for the edda demo project.
# Creates a pre-populated .edda/ workspace with decisions, notes, and commits.
#
# Usage:
#   cd examples/demo-project
#   bash setup.sh
#
# After setup, try:
#   edda ask "database"
#   edda ask "auth"
#   edda ask
#   edda log
#   edda context
#   edda watch

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
cd "$SCRIPT_DIR"

# Use EDDA env var, or workspace debug build, or edda from PATH
if [ -z "${EDDA:-}" ]; then
  if [ -x "../../target/debug/edda" ] || [ -x "../../target/debug/edda.exe" ]; then
    EDDA="../../target/debug/edda"
  elif command -v edda &>/dev/null; then
    EDDA="edda"
  else
    echo "edda not found. Run 'cargo build --bin edda' first, or set EDDA=/path/to/edda"
    exit 1
  fi
fi

# Clean previous run
if [ -d .edda ]; then
  echo "Removing existing .edda/ ..."
  rm -rf .edda
fi

# Initialize workspace (skip hooks — this is a standalone demo)
"$EDDA" init --no-hooks
echo ""

# ── Session 1: Project kickoff ──────────────────────────────────────

"$EDDA" note "Project kickoff: building a task management API" --tag session

"$EDDA" decide "lang=Rust" --reason "performance, memory safety, strong type system"
"$EDDA" decide "db.engine=SQLite" --reason "single-user MVP, zero deployment overhead, FTS5 for search"
"$EDDA" decide "api.style=REST" --reason "client SDK compatibility, team familiarity"

"$EDDA" note "Explored GraphQL but REST is simpler for the MVP scope"
"$EDDA" commit --title "feat: initial project scaffold" --purpose "setup Cargo workspace, add axum + rusqlite deps"

echo "Session 1: project kickoff recorded"

# ── Session 2: Auth decisions ───────────────────────────────────────

"$EDDA" note "Starting auth implementation" --tag session

"$EDDA" decide "auth.method=JWT" --reason "stateless, scales horizontally, no session store needed"
"$EDDA" decide "auth.token_expiry=15min" --reason "short-lived for security, refresh token handles renewal"
"$EDDA" decide "auth.refresh=httpOnly_cookie" --reason "XSS-resistant, automatic on every request"

"$EDDA" note "Considered session-based auth but JWT avoids shared state between instances"
"$EDDA" commit --title "feat: add JWT auth middleware" --purpose "implement login, validate, refresh token flow"

echo "Session 2: auth decisions recorded"

# ── Session 3: Caching & DB evolution ───────────────────────────────

"$EDDA" note "Revisiting database choice after load testing" --tag session

"$EDDA" decide "cache.strategy=None" --reason "premature optimization, revisit after benchmarks"
"$EDDA" note "Load test results: SQLite handles 500 req/s easily for our scale"

"$EDDA" commit --title "perf: add connection pooling" --purpose "reduce SQLite lock contention under concurrent reads"
"$EDDA" commit --title "feat: add task CRUD endpoints" --purpose "implement create, read, update, delete for tasks"

echo "Session 3: caching & performance recorded"

# ── Session 4: Supersede a decision ─────────────────────────────────

"$EDDA" note "User growth exceeded expectations, need real caching" --tag session

"$EDDA" decide "cache.strategy=Redis" --reason "need TTL, pub/sub for cache invalidation, 10x read throughput"
"$EDDA" decide "deploy.container=Docker" --reason "reproducible builds, easy Redis sidecar"

"$EDDA" note "Benchmarked memcached vs Redis: Redis pub/sub for invalidation is worth the extra memory"
"$EDDA" commit --title "feat: add Redis caching layer" --purpose "cache hot task queries, 50ms -> 5ms p95 latency"

echo "Session 4: cache upgrade recorded"

# ── Done ────────────────────────────────────────────────────────────

echo ""
echo "Demo workspace ready! Try these commands:"
echo ""
echo "  edda ask                    # overview of all active decisions"
echo "  edda ask database           # search for database-related decisions"
echo "  edda ask cache              # see cache decision timeline (supersede chain)"
echo "  edda ask auth               # browse auth domain"
echo "  edda ask db.engine          # exact key lookup"
echo "  edda ask --json             # JSON output for LLM consumption"
echo "  edda log                    # full event log"
echo "  edda log --type decision    # only decisions"
echo "  edda context                # what an agent sees at session start"
echo "  edda watch                  # real-time TUI (Ctrl+C to exit)"
echo ""
