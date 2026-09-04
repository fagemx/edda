//! Test-only helpers shared by the `cmd_*` test modules.

use edda_store::test_support::IsolatedStoreRoot;

/// Point the per-user store at a throwaway directory for this test.
///
/// Anything that writes to the store — `edda init` and `edda group` both call
/// `registry::register_project` — must be wrapped in this, or it writes into the
/// developer's real `registry.json` and stays there (GH-417). CI never notices,
/// because its containers start empty; only the developer's machine accumulates.
///
/// GH-757: the override is a thread-local installed by `edda-store`'s
/// test-support API. `edda_store::store_root()` on this thread resolves into
/// the private directory; every other thread keeps its own resolution, and the
/// process environment is never mutated — so a panicking test cannot strand
/// its siblings on a directory that is about to be deleted, and a concurrently
/// running test cannot resolve into this test's root. Spawned subprocesses
/// inherit only the real environment: a child that must use an isolated store
/// is passed `EDDA_STORE_ROOT` explicitly via `Command::env`.
///
/// Keep the returned value alive for the whole test:
///
/// ```ignore
/// let _store = test_support::isolated_store();
/// ```
pub(crate) fn isolated_store() -> IsolatedStoreRoot {
    edda_store::test_support::isolated_store_root().expect("isolated store")
}

// Guard-restore semantics (RAII on drop, panic safety, thread locality) are
// tested at the source in `edda-store::test_support`; a duplicate here could
// only re-prove them through an extra indirection.
