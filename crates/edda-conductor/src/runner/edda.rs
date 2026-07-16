//! Edda (edda) integration helpers for the Conductor.
//!
//! All operations are best-effort: if `edda` is not in PATH or the command
//! fails, the Conductor continues without context injection. This keeps
//! Edda optional — the Conductor works as a plain task runner without it.

use std::path::Path;
use std::process::Command;

/// Ensure `.edda/` ledger exists in the working directory.
/// No-op if already initialized or if `edda` is not available.
///
/// This shells out to the *installed* `edda`, and `edda init` registers its cwd
/// in the operator's global project registry. That is right in production and
/// wrong under test: the runner's tests each hand this a fresh
/// `tempfile::tempdir()`, so every test run filed another dead temp path in the
/// real registry — 13 per `cargo test -p edda-conductor`, which is GH-417.
///
/// The tests are isolated at this seam rather than in each test body: there is
/// one call site and thirteen callers today, so guarding the callers would be
/// thirteen chances for the fourteenth to forget. `EDDA_STORE_ROOT` points the
/// child at a throwaway store — the whole path still runs, it just cannot reach
/// the operator's registry. Whole-process isolation is safe here, which it
/// usually is not: the variable is process-wide, but no test in this crate wants
/// the real store.
pub fn ensure_init(cwd: &Path) {
    if cwd.join(".edda").exists() {
        return;
    }
    #[cfg(test)]
    let _isolation = tests::isolate_store_for_this_process();

    let _ = Command::new("edda")
        .arg("init")
        .current_dir(cwd)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
}

/// Run `edda context` and return the output as a string.
/// Returns empty string if edda is not available or fails.
pub fn get_context(cwd: &Path) -> String {
    Command::new("edda")
        .arg("context")
        .current_dir(cwd)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default()
}

/// Record a note to the edda ledger.
/// Best-effort: silently ignores failures.
pub fn record_note(cwd: &Path, text: &str, tags: &[&str]) {
    let mut cmd = Command::new("edda");
    cmd.arg("note").arg(text).current_dir(cwd);
    for tag in tags {
        cmd.arg("--tag").arg(tag);
    }
    cmd.stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    let _ = cmd.status();
}

/// Truncate a string to at most `max` bytes on a valid UTF-8 char boundary.
fn truncate_str(s: &str, max: usize) -> &str {
    if s.len() <= max {
        return s;
    }
    // Walk backwards from max to find a char boundary
    let mut end = max;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

/// Record a phase completion event.
pub fn record_phase_done(cwd: &Path, phase_id: &str, summary: Option<&str>, cost_usd: Option<f64>) {
    let cost_str = cost_usd.map(|c| format!(" [${c:.3}]")).unwrap_or_default();
    let summary_str = summary
        .map(|s| {
            let s = s.trim();
            if s.len() > 300 {
                format!(": {}...", truncate_str(s, 297))
            } else if s.is_empty() {
                String::new()
            } else {
                format!(": {s}")
            }
        })
        .unwrap_or_default();
    let text = format!("Phase \"{phase_id}\" passed{cost_str}{summary_str}");
    record_note(cwd, &text, &["conductor", &format!("phase:{phase_id}")]);
}

/// Record a phase failure event.
pub fn record_phase_failed(cwd: &Path, phase_id: &str, error: &str) {
    let error_str = if error.len() > 200 {
        format!("{}...", truncate_str(error, 200))
    } else {
        error.to_string()
    };
    let text = format!("Phase \"{phase_id}\" failed: {error_str}");
    record_note(
        cwd,
        &text,
        &["conductor", &format!("phase:{phase_id}"), "failure"],
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Point every `edda` child this test binary spawns at a throwaway store.
    ///
    /// Returns a guard the caller holds for the duration of the spawn. The store
    /// itself is created once and leaked: it must outlive every test in the
    /// process, and tests run in parallel threads, so there is no later moment
    /// at which dropping it would be safe. It is an OS temp dir — the OS
    /// reclaims it.
    ///
    /// The lock is what makes this correct rather than merely usual:
    /// `set_var` is process-wide, so a concurrent test that reads
    /// `EDDA_STORE_ROOT` mid-write would see a torn value. Everything here wants
    /// the same store, so serialising the spawns costs nothing worth having.
    pub(super) fn isolate_store_for_this_process() -> std::sync::MutexGuard<'static, ()> {
        use std::sync::{Mutex, Once, OnceLock};
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        static SET: Once = Once::new();

        SET.call_once(|| {
            let store = tempfile::tempdir().expect("temp store for edda-conductor tests");
            std::env::set_var("EDDA_STORE_ROOT", store.path());
            std::mem::forget(store);
        });
        LOCK.get_or_init(|| Mutex::new(()))
            .lock()
            .unwrap_or_else(|e| e.into_inner())
    }

    /// GH-417: the runner shells out to the installed `edda`, and `edda init`
    /// registers its cwd. Under test that cwd is a `tempfile::tempdir()`, so
    /// every run filed another dead path in the operator's real registry — 13
    /// per `cargo test -p edda-conductor`, forever, on the machine of whoever
    /// ran the tests.
    ///
    /// Drives `ensure_init` itself and checks where its child landed. An
    /// earlier version of this test called the isolation helper directly and
    /// asserted it worked — which it always did, with or without `ensure_init`
    /// wired to it, so deleting the fix left the test green. Guarding the fix
    /// means going through the function the fix is in.
    #[test]
    fn ensure_init_sends_its_child_to_a_throwaway_store() {
        let dir = tempfile::tempdir().unwrap();
        ensure_init(dir.path());

        // Best-effort by design: with no `edda` on PATH nothing spawns, and
        // there is no child to have misdirected. CI has no edda installed; the
        // developer's machine does, and that is where the damage lands.
        if !dir.path().join(".edda").exists() {
            return;
        }

        let root = std::env::var("EDDA_STORE_ROOT")
            .expect("ensure_init must isolate the store before it spawns anything");

        // Checked against the OS temp dir, not against `store_root()`: that
        // function *returns* EDDA_STORE_ROOT when set, so asking it here would
        // compare the temp store with itself and pass for the wrong reason.
        assert!(
            Path::new(&root).starts_with(std::env::temp_dir()),
            "the child must write to a throwaway store, got: {root}"
        );
        // `edda init` writes `registry.json` under whatever store root it
        // resolves. Finding it here is what proves the child registered into
        // the throwaway rather than into the operator's own.
        assert!(
            Path::new(&root).join("registry.json").exists(),
            "the child registered somewhere other than the throwaway store: {root}"
        );
    }

    #[test]
    fn get_context_returns_empty_on_missing_edda_dir() {
        let dir = tempfile::tempdir().unwrap();
        let result = get_context(dir.path());
        // Either empty (no .edda/) or actual context if edda is in PATH
        // This test just verifies it doesn't panic
        assert!(result.is_empty() || result.contains("CONTEXT"));
    }

    #[test]
    fn truncate_str_ascii() {
        assert_eq!(truncate_str("hello", 10), "hello");
        assert_eq!(truncate_str("hello world", 5), "hello");
    }

    #[test]
    fn truncate_str_multibyte() {
        // "café" = [99, 97, 102, 195, 169] — 'é' is 2 bytes
        let s = "café";
        assert_eq!(s.len(), 5);
        // Truncate at 4 would land inside 'é', should back up to 3
        assert_eq!(truncate_str(s, 4), "caf");
        assert_eq!(truncate_str(s, 5), "café");
    }

    #[test]
    fn truncate_str_cjk() {
        // Each CJK char is 3 bytes
        let s = "你好世界";
        assert_eq!(s.len(), 12);
        // 7 bytes = 2 full chars (6) + 1 byte into 3rd char → back to 6
        assert_eq!(truncate_str(s, 7), "你好");
        assert_eq!(truncate_str(s, 6), "你好");
    }
}
