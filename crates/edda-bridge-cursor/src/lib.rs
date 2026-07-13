//! Cursor Agent bridge for Edda.

mod admin;
mod dispatch;
mod parse;

pub use admin::{doctor, install, uninstall};
pub use dispatch::{hook_entrypoint_from_stdin, HookResult};
