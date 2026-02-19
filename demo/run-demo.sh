#!/usr/bin/env bash
# ─────────────────────────────────────────────────────
# Edda Conductor Demo — tmux split-screen showcase
# ─────────────────────────────────────────────────────
#
# Layout:
#   ┌───────────────────────┬──────────────────────┐
#   │                       │                      │
#   │  Conductor running    │  File tree (live)    │
#   │  (plan execution)     │                      │
#   │                       ├──────────────────────┤
#   │                       │                      │
#   │                       │  edda log (live)     │
#   │                       │                      │
#   └───────────────────────┴──────────────────────┘
#
# Usage:
#   chmod +x run-demo.sh
#   ./run-demo.sh [plan.yaml]
#
# Prerequisites:
#   - tmux
#   - edda (edda) binary in PATH
#   - claude CLI logged in
#   - watch command (standard on Linux, `brew install watch` on macOS)

set -euo pipefail

PLAN="${1:-plan.yaml}"
DEMO_DIR="$(mktemp -d -t edda-demo-XXXX)"
SESSION="edda-demo"

echo "📁 Demo workspace: $DEMO_DIR"
cp "$PLAN" "$DEMO_DIR/plan.yaml"

# Kill existing session if any
tmux kill-session -t "$SESSION" 2>/dev/null || true

# Create tmux session with main pane (conductor)
tmux new-session -d -s "$SESSION" -c "$DEMO_DIR" -x 200 -y 50

# Split right: file tree watcher (top right)
tmux split-window -h -t "$SESSION" -c "$DEMO_DIR"
tmux send-keys -t "$SESSION:0.1" \
  "watch -n 1 'find . -not -path \"./target/*\" -not -path \"./.edda/*\" -not -name \"*.lock\" -type f | sort | head -30'" C-m

# Split bottom-right: edda log watcher
tmux split-window -v -t "$SESSION:0.1" -c "$DEMO_DIR"
tmux send-keys -t "$SESSION:0.2" \
  "watch -n 2 'edda log --last 10 2>/dev/null || echo \"(waiting for edda events...)\"'" C-m

# Resize: left pane 60%, right panes 40%
tmux resize-pane -t "$SESSION:0.0" -x "60%"

# Style: set pane borders and titles
tmux set-option -t "$SESSION" pane-border-status top
tmux select-pane -t "$SESSION:0.0" -T "🎬 Conductor"
tmux select-pane -t "$SESSION:0.1" -T "📂 Files"
tmux select-pane -t "$SESSION:0.2" -T "📖 Edda Log"

# Focus on the conductor pane
tmux select-pane -t "$SESSION:0.0"

# Type the command but don't execute yet (let the user hit Enter for the recording)
tmux send-keys -t "$SESSION:0.0" "edda conduct run plan.yaml" ""

echo ""
echo "┌─────────────────────────────────────────────┐"
echo "│  Edda Conductor Demo Ready                  │"
echo "│                                             │"
echo "│  tmux attach -t $SESSION              │"
echo "│                                             │"
echo "│  Press Enter in the left pane to start.     │"
echo "│  Ctrl+B then D to detach.                   │"
echo "│  Ctrl+B then Z to zoom a pane.              │"
echo "└─────────────────────────────────────────────┘"
echo ""
echo "🎬 To start recording (asciinema):"
echo "   asciinema rec demo.cast -c 'tmux attach -t $SESSION'"
echo ""

# Attach
tmux attach -t "$SESSION"
