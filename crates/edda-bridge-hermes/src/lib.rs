//! Hermes agent bridge for edda.
//!
//! Hermes ships three hook systems; this bridge targets **Shell Hooks** — the
//! stdin/stdout JSON layer registered under the `hooks:` key of
//! `~/.hermes/cli-config.yaml` (or `~/.hermes/config.yaml`). Shell hooks are
//! peer to the Codex and Claude Code contracts, so most of `bridge-claude`'s
//! machinery (peers, pack, render, state, nudge, digest) is reused verbatim.
//!
//! Wire protocol (from `hermes-agent/agent/shell_hooks.py`, not just docs):
//!
//! ```text
//! stdin:  {"hook_event_name", "tool_name", "tool_input", "session_id", "cwd", "extra"}
//! stdout — pre_tool_call:   {"decision":"block","reason":"..."}      (Claude-shape accepted)
//! stdout — pre_llm_call:    {"context":"injected text"}
//! stdout — pre_verify:      {"action":"continue","message":"..."}
//! stdout — no-op:           {} (or empty)
//! ```
//!
//! Hermes-only events we route:
//!   * `pre_verify`     — verification gate (has no Claude/Codex equivalent)
//!   * `on_session_reset` — `/new` in session (auto-digest hook)
//!
//! Not yet wired (skeleton): `pre_gateway_dispatch`, `transform_*`,
//! `pre_approval_request`/`post_approval_response`, `pre_api_request`.

mod admin;
mod dispatch;
mod parse;

pub use admin::{doctor, install, uninstall};
pub use dispatch::{hook_entrypoint_from_stdin, HookResult};
