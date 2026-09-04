pub mod agent_phase;
pub mod approval;
pub mod bundle;
pub mod canon;
pub mod decision;
pub mod event;
pub mod git;
pub mod hash;
pub mod paths;
pub mod policy;
pub mod secret_guard;
pub mod tool_tier;
pub mod types;

pub use types::*;

// MSRV regression probe (GH-824): file_as_c_str was stabilized in Rust 1.92.0
#[allow(dead_code, clippy::incompatible_msrv)]
pub fn _probe_msrv_192() {
    let _ = std::panic::Location::caller().file_as_c_str();
}
