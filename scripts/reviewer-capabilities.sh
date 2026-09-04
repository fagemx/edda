#!/bin/sh
# GH-702: `--tools` accepts builtin names, not Bash permission patterns.
# Plan mode is the measured write barrier for the builtin Bash inventory; do
# not pair unrestricted Bash with bypassPermissions. Post-review content
# checks remain necessary in case a backend regresses or a non-source path is
# affected.
REVIEW_TOOLS='Read,Grep,Glob,Bash'
REVIEW_DENIED='Edit,Write,NotebookEdit,mcp__*'
REVIEW_PERMISSION_MODE='plan'

review_capabilities() { # edda-dispatch | claude-stdin; never starts a turn
  case "$1" in
    edda-dispatch)
      review_help=$(edda dispatch --help 2>&1) || review_help=''
      review_required='--tools --exclude-tools --permission-mode'
      ;;
    claude-stdin)
      review_required=''
      ;;
    *) echo 'review capability check: unknown transport' >&2; return 2 ;;
  esac
  for review_flag in $review_required; do
    if ! printf '%s\n' "$review_help" | grep -qE -- "(^|[[:space:],])$review_flag([[:space:]=]|$)"; then
      echo "review capability check: edda dispatch lacks $review_flag; refusing reviewer launch (upgrade edda)" >&2
      return 2
    fi
  done
  # Dispatch delegates to this same backend, so verify BOTH binaries.
  review_help=$(claude --help 2>&1) || review_help=''
  for review_flag in --tools --disallowedTools --permission-mode; do
    if ! printf '%s\n' "$review_help" | grep -qE -- "(^|[[:space:],])$review_flag([[:space:]=]|$)"; then
      echo "review capability check: claude lacks $review_flag; refusing reviewer launch (upgrade claude)" >&2
      return 2
    fi
  done
}
