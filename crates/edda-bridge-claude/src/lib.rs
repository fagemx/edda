pub mod digest;
pub mod pattern;
pub mod peers;
pub mod redact;
pub mod render;
pub mod state;
pub mod watch;

mod admin;
mod dispatch;
mod narrative;
pub mod nudge;
mod parse;
mod plan;
mod signals;

// Re-export public API (CLI consumers unchanged)
pub use admin::{doctor, install, uninstall};
pub use dispatch::{hook_entrypoint_from_stdin, HookResult};
