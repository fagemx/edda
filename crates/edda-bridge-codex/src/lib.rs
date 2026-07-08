//! Codex CLI bridge for edda.
//!
//! Handles the subset of Codex hook events that carry equivalent semantics to
//! Claude Code's events:
//!
//! | Codex event         | edda action                                             |
//! |---------------------|---------------------------------------------------------|
//! | SessionStart        | inject doctrine + hot pack + workspace context          |
//! | UserPromptSubmit    | inject lightweight workspace context (peer diff)        |
//! | PreToolUse          | evaluate L3 rules against the pending command           |
//! | PostToolUse         | detect decision signals, emit nudges                    |
//! | SessionEnd / Stop   | auto-digest, cleanup                                    |
//! | SubagentStart/Stop  | write / clear sub-agent heartbeat                       |
//! | PermissionRequest   | Codex-only: L3 can decline permission at the gate       |
//!
//! Design rule: this crate reuses `edda-bridge-claude`'s shared machinery
//! (peers, pack, digest, render, state, postmortem hooks) — none of that is
//! Claude-specific. Only the parse/dispatch/admin layer is bridge-specific.

mod admin;
mod dispatch;
mod parse;

pub use admin::{doctor, install, uninstall};
pub use dispatch::{hook_entrypoint_from_stdin, HookResult};
