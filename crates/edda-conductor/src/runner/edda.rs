//! Edda (edda) integration helpers for the Conductor.
//!
//! Ledger *writes* go through the `edda_ledger` library directly — never by
//! shelling out to a PATH `edda` binary (GH-584): the binary on PATH can be
//! an older build silently running old behavior, and conductor already
//! depends on the library for reads. A failed write is never swallowed: it
//! is reported on stderr so "no conductor events in the ledger" is always
//! distinguishable from "the write is broken".
//!
//! Two operations still shell out to the installed `edda` on purpose:
//! [`ensure_init`] (full init also registers the cwd in the operator's
//! global project registry, which is right in production) and [`get_context`]
//! (a read-only convenience). All operations are best-effort: if `edda` is
//! not in PATH or the command fails, the Conductor continues without context
//! injection. This keeps Edda optional — the Conductor works as a plain task
//! runner without it.

use anyhow::Context;
use edda_core::event::new_note_event;
use edda_core::secret_guard::redact;
use edda_ledger::lock::WorkspaceLock;
use edda_ledger::Ledger;
use std::path::Path;
use std::process::Command;

/// Role stamped on the note events the conductor writes about its own runs.
/// The conductor is an automated writer — never the operator.
const ROLE: &str = "agent";

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

/// Append a `note` event to the workspace ledger at `cwd`, via the
/// `edda_ledger` library (GH-584 problem 3: no PATH `edda` child process).
///
/// `structured` optionally embeds a JSON object under a dedicated payload
/// key (same pattern as decision notes embed `payload["decision"]`). This
/// is the report-facing shape; the human-readable `text` stays in
/// `payload["text"]` alongside it.
fn append_ledger_note(
    cwd: &Path,
    text: &str,
    tags: &[String],
    structured: Option<(&str, serde_json::Value)>,
) -> anyhow::Result<()> {
    let ledger = Ledger::open(cwd).context("opening workspace ledger")?;
    let _lock = WorkspaceLock::acquire(&ledger.paths).context("acquiring workspace lock")?;
    let branch = ledger.head_branch().context("reading HEAD branch")?;
    let parent_hash = ledger
        .last_event_hash()
        .context("reading last event hash")?;

    // EDDA-SECRET-GUARD1 q331: same scrub the `edda note` CLI applies.
    let (safe_text, hits) = redact(text);
    if !hits.is_empty() {
        eprintln!(
            "⚠ secret-guard: redacted {n} secret pattern(s) before writing conductor NOTE ({kinds})",
            n = hits.len(),
            kinds = hits.iter().map(|h| h.kind).collect::<Vec<_>>().join(", ")
        );
    }

    let mut event = new_note_event(&branch, parent_hash.as_deref(), ROLE, &safe_text, tags)
        .context("building note event")?;
    if let Some((key, value)) = structured {
        event.payload[key] = value;
        // Embedding the structured payload changes the event body — re-hash
        // exactly like `new_decision_event` does for `payload["decision"]`,
        // or the append rejects the event as hash-invalid.
        edda_core::event::finalize_event(&mut event).context("re-finalizing note event")?;
    }
    ledger
        .append_event(&event)
        .context("appending note event")?;

    // GH-584 review round 2 P1-5: parity with `edda note` (cmd_note.rs) —
    // refresh the derived markdown views so an operator reading
    // `.edda/views/<branch>/log.md` sees the note immediately, not only
    // after the next rebuild. Same best-effort pattern: a failed refresh
    // never blocks a successful write.
    let _ = edda_derive::rebuild_branch(&ledger, &branch);

    Ok(())
}

/// Best-effort wrapper (GH-584 problem 2): a failed ledger write is never
/// silent — it is reported on stderr, so a shrinking conductor presence in
/// the workspace ledger is observable instead of indistinguishable from
/// "the phase simply produced no note".
fn append_ledger_note_best_effort(
    subject: &str,
    cwd: &Path,
    text: &str,
    tags: &[String],
    structured: Option<(&str, serde_json::Value)>,
) {
    if let Err(e) = append_ledger_note(cwd, text, tags, structured) {
        eprintln!("⚠ edda-conductor: workspace ledger write failed for {subject}: {e:#}");
    }
}

/// Record a note to the workspace ledger.
/// Best-effort: a failed write is reported on stderr, never swallowed (GH-584).
pub fn record_note(cwd: &Path, text: &str, tags: &[&str]) {
    let tags: Vec<String> = tags.iter().map(|t| t.to_string()).collect();
    append_ledger_note_best_effort("note", cwd, text, &tags, None);
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
///
/// See [`record_phase_done_with_plan`] — this is the plan-less wrapper kept
/// for call sites that do not have the plan in scope (GH-584).
pub fn record_phase_done(cwd: &Path, phase_id: &str, summary: Option<&str>, cost_usd: Option<f64>) {
    record_phase_done_with_plan(cwd, None, phase_id, summary, cost_usd);
}

/// [`record_phase_done`] with the plan name, so the structured ledger
/// payload carries `plan_id`. Call sites that have the plan in scope should
/// prefer this variant (GH-584).
///
/// The phase terminal state reaches the workspace ledger as a note event
/// with a structured `conductor_phase` payload:
/// `{ plan_id, phase_id, status, cost_usd }`. Along #533's discipline,
/// `cost_usd: None` (unmeasured) serializes as JSON **null** — never the
/// 0.0 sentinel; a measured 0.0 stays 0.0. The human-readable text is kept
/// alongside the structured fields, but the structured fields are the
/// report-facing surface.
pub fn record_phase_done_with_plan(
    cwd: &Path,
    plan_id: Option<&str>,
    phase_id: &str,
    summary: Option<&str>,
    cost_usd: Option<f64>,
) {
    record_phase_done_timed(cwd, plan_id, phase_id, summary, cost_usd, None);
}

/// Phase receipt with optional measured elapsed time, exposed by `edda log --json`.
pub fn record_phase_done_timed(
    cwd: &Path,
    plan_id: Option<&str>,
    phase_id: &str,
    summary: Option<&str>,
    cost_usd: Option<f64>,
    elapsed_ms: Option<u64>,
) {
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
    let text = format!(
        "{text} [elapsed: {}]",
        elapsed_ms
            .map(|ms| format!("{ms} ms"))
            .unwrap_or_else(|| "—".into())
    );
    let tags = vec!["conductor".to_string(), format!("phase:{phase_id}")];
    let payload = serde_json::json!({
        "plan_id": plan_id,
        "phase_id": phase_id,
        "status": "passed",
        "cost_usd": cost_usd,
        "elapsed_ms": elapsed_ms,
        "elapsed_measured": elapsed_ms.is_some(),
    });
    append_ledger_note_best_effort(
        &format!("phase \"{phase_id}\" passed"),
        cwd,
        &text,
        &tags,
        Some(("conductor_phase", payload)),
    );
}

/// Record a phase failure event.
///
/// See [`record_phase_failed_with_plan`] — this is the plan-less wrapper
/// kept for call sites that do not have the plan in scope (GH-584).
pub fn record_phase_failed(cwd: &Path, phase_id: &str, error: &str) {
    record_phase_failed_with_plan(cwd, None, phase_id, None, error);
}

/// [`record_phase_failed`] with the plan name and the phase's measured cost,
/// so the structured ledger payload carries `plan_id` and an honest
/// `cost_usd`. Call sites that have the plan in scope should prefer this
/// variant (GH-584). `cost_usd: None` (unmeasured) serializes as JSON null —
/// never the 0.0 sentinel (#533 discipline); a measured failure keeps its
/// cost instead of being rewritten as unmeasured.
pub fn record_phase_failed_with_plan(
    cwd: &Path,
    plan_id: Option<&str>,
    phase_id: &str,
    cost_usd: Option<f64>,
    error: &str,
) {
    record_phase_failed_timed(cwd, plan_id, phase_id, cost_usd, error, None);
}

/// Failed phase receipt; missing timing remains unmeasured, including pre-run failures.
pub fn record_phase_failed_timed(
    cwd: &Path,
    plan_id: Option<&str>,
    phase_id: &str,
    cost_usd: Option<f64>,
    error: &str,
    elapsed_ms: Option<u64>,
) {
    let error_str = if error.len() > 200 {
        format!("{}...", truncate_str(error, 200))
    } else {
        error.to_string()
    };
    let cost_str = cost_usd.map(|c| format!(" [${c:.3}]")).unwrap_or_default();
    let text = format!("Phase \"{phase_id}\" failed{cost_str}: {error_str}");
    let text = format!(
        "{text} [elapsed: {}]",
        elapsed_ms
            .map(|ms| format!("{ms} ms"))
            .unwrap_or_else(|| "—".into())
    );
    let mut tags = vec!["conductor".to_string(), format!("phase:{phase_id}")];
    tags.push("failure".to_string());
    let payload = serde_json::json!({
        "plan_id": plan_id,
        "phase_id": phase_id,
        "status": "failed",
        "cost_usd": cost_usd,
        "elapsed_ms": elapsed_ms,
        "elapsed_measured": elapsed_ms.is_some(),
    });
    append_ledger_note_best_effort(
        &format!("phase \"{phase_id}\" failed"),
        cwd,
        &text,
        &tags,
        Some(("conductor_phase", payload)),
    );
}

/// Record a gate timeout in the workspace ledger with the honest
/// classification (GH-552): the phase's work completed and its checks
/// passed — nothing failed — so the note carries `status:
/// "gate_timed_out"`, never "failed".
pub fn record_phase_gate_timed_out(
    cwd: &Path,
    plan_id: Option<&str>,
    phase_id: &str,
    cost_usd: Option<f64>,
    error: &str,
) {
    let error_str = if error.len() > 200 {
        format!("{}...", truncate_str(error, 200))
    } else {
        error.to_string()
    };
    let cost_str = cost_usd.map(|c| format!(" [${c:.3}]")).unwrap_or_default();
    let text = format!("Phase \"{phase_id}\" gate timed out{cost_str}: {error_str}");
    let mut tags = vec!["conductor".to_string(), format!("phase:{phase_id}")];
    tags.push("gate_timeout".to_string());
    let payload = serde_json::json!({
        "plan_id": plan_id,
        "phase_id": phase_id,
        "status": "gate_timed_out",
        "cost_usd": cost_usd,
        "elapsed_ms": null,
        "elapsed_measured": false,
    });
    append_ledger_note_best_effort(
        &format!("phase \"{phase_id}\" gate timed out"),
        cwd,
        &text,
        &tags,
        Some(("conductor_phase", payload)),
    );
}

/// Record plan completion in the workspace ledger with the honest total
/// cost: `total_cost_usd: None` (unmeasured — no phase ever recorded a
/// measured cost) serializes as JSON null, never 0.0 (#533 discipline).
pub fn record_plan_completed(cwd: &Path, plan_id: &str, total_cost_usd: Option<f64>) {
    let cost_str = total_cost_usd
        .map(|c| format!(" [${c:.3}]"))
        .unwrap_or_default();
    let text = format!("Plan \"{plan_id}\" completed{cost_str}");
    let tags = vec!["conductor".to_string(), format!("plan:{plan_id}")];
    let payload = serde_json::json!({
        "plan_id": plan_id,
        "status": "completed",
        "total_cost_usd": total_cost_usd,
    });
    append_ledger_note_best_effort(
        &format!("plan \"{plan_id}\" completed"),
        cwd,
        &text,
        &tags,
        Some(("conductor_plan", payload)),
    );
}

/// Record plan abort in the workspace ledger (structured
/// `conductor_plan` payload; see [`record_plan_completed`]).
pub fn record_plan_aborted(cwd: &Path, plan_id: &str, phases_passed: usize, phases_pending: usize) {
    let text =
        format!("Plan \"{plan_id}\" aborted ({phases_passed} passed, {phases_pending} pending)");
    let mut tags = vec!["conductor".to_string(), format!("plan:{plan_id}")];
    tags.push("aborted".to_string());
    let payload = serde_json::json!({
        "plan_id": plan_id,
        "status": "aborted",
        "phases_passed": phases_passed,
        "phases_pending": phases_pending,
    });
    append_ledger_note_best_effort(
        &format!("plan \"{plan_id}\" aborted"),
        cwd,
        &text,
        &tags,
        Some(("conductor_plan", payload)),
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
    ///
    /// The lock is the *shared* `CLAIM_ENV_LOCK`, not a private one (review
    /// round 2): a second private mutex here could relocate the root while a
    /// heartbeat test held `ClaimEnvGuard`, recreating the ordering-dependent
    /// cross-store write/read split the Windows-CI fix closed. Reentrancy
    /// caveat: a caller already holding `ClaimEnvGuard` must not reach this
    /// (`std::sync::Mutex` is not reentrant). The only production-path caller
    /// is `ensure_init`, and its test callers that run under `ClaimEnvGuard`
    /// pre-mark the cwd with `.edda` (see `sequential::tests::make_repo`), so
    /// `ensure_init` early-returns before the lock is taken.
    pub(super) fn isolate_store_for_this_process() -> std::sync::MutexGuard<'static, ()> {
        use std::sync::Once;
        static SET: Once = Once::new();

        let lock = crate::runner::sequential::tests::CLAIM_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        SET.call_once(|| {
            let store = tempfile::tempdir().expect("temp store for edda-conductor tests");
            std::env::set_var("EDDA_STORE_ROOT", store.path());
            std::mem::forget(store);
        });
        lock
    }

    /// P1 regression (review round 2): the store-isolation redirect must
    /// serialize on the *shared* `CLAIM_ENV_LOCK`, not a private mutex. With
    /// the old private lock, this helper could relocate `EDDA_STORE_ROOT`
    /// while a heartbeat test held `ClaimEnvGuard`, splitting writes and
    /// reads across two stores. Here `ClaimEnvGuard` is held while the
    /// helper runs on another thread: it must stay blocked (no relocation,
    /// not finished) until the guard drops.
    #[test]
    fn store_isolation_uses_the_shared_claim_env_lock() {
        use crate::runner::sequential::tests::ClaimEnvGuard;

        let guard = ClaimEnvGuard::new();
        let before = std::env::var_os("EDDA_STORE_ROOT")
            .expect("ClaimEnvGuard must have redirected the root");
        let handle = std::thread::spawn(|| {
            drop(super::tests::isolate_store_for_this_process());
        });
        std::thread::sleep(std::time::Duration::from_millis(300));
        // With the old private lock this thread either relocated the root
        // under us (Once unfired) or returned immediately (Once fired).
        assert_eq!(
            std::env::var_os("EDDA_STORE_ROOT").as_deref(),
            Some(before.as_os_str()),
            "store isolation must not relocate EDDA_STORE_ROOT while ClaimEnvGuard holds the shared lock"
        );
        assert!(
            !handle.is_finished(),
            "store isolation must block on CLAIM_ENV_LOCK while ClaimEnvGuard is held"
        );
        drop(guard);
        handle.join().expect("isolation helper must not panic");
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

    // ── GH-584 regression tests ─────────────────────────────────────

    /// Initialize a throwaway workspace ledger so direct `edda_ledger` writes
    /// have a workspace to land in (the runner's `ensure_init` normally does
    /// this at plan start).
    fn init_test_ledger(dir: &Path) {
        edda_ledger::Ledger::ensure_initialized(dir).expect("init test workspace ledger");
    }

    /// All `note` events currently in the throwaway workspace ledger.
    fn note_events(dir: &Path) -> Vec<edda_core::Event> {
        edda_ledger::Ledger::open(dir)
            .expect("open test workspace ledger")
            .iter_events_by_type("note")
            .expect("read note events")
    }

    /// The one conductor phase event written so far, if any.
    fn conductor_phase_event(dir: &Path) -> Option<edda_core::Event> {
        note_events(dir)
            .into_iter()
            .find(|e| e.payload.get("conductor_phase").is_some())
    }

    /// GH-584 problem 1: measured phase cost must reach the workspace ledger
    /// as a structured `cost_usd` field, not as `[$0.123]` prose.
    #[test]
    fn phase_done_writes_a_structured_cost_field_into_the_workspace_ledger() {
        // Pre-fix this test spawns a PATH `edda`; redirect its store so it
        // cannot touch the operator's registry (GH-417 seam).
        let _isolation = isolate_store_for_this_process();
        let dir = tempfile::tempdir().unwrap();
        init_test_ledger(dir.path());

        record_phase_done(dir.path(), "build", Some("compiled cleanly"), Some(0.123));

        let event = conductor_phase_event(dir.path())
            .expect("phase done must write a structured conductor_phase payload to the ledger");
        let payload = &event.payload["conductor_phase"];
        assert_eq!(payload["phase_id"], "build");
        assert_eq!(payload["status"], "passed");
        assert_eq!(
            payload["cost_usd"],
            serde_json::json!(0.123),
            "measured cost must be a structured numeric field"
        );
    }

    /// GH-584 problem 1 + #533 discipline: unmeasured cost must be JSON null
    /// — never the 0.0 sentinel, and never an absent event.
    #[test]
    fn unmeasured_phase_cost_is_null_not_a_zero_sentinel() {
        let _isolation = isolate_store_for_this_process();
        let dir = tempfile::tempdir().unwrap();
        init_test_ledger(dir.path());

        record_phase_done(dir.path(), "probe", None, None);

        let event = conductor_phase_event(dir.path())
            .expect("unmeasured phase must still write a structured event (null cost)");
        let payload = &event.payload["conductor_phase"];
        assert_eq!(payload["phase_id"], "probe");
        assert_eq!(payload["status"], "passed");
        assert!(
            payload["cost_usd"].is_null(),
            "unmeasured cost must serialize as null, got: {}",
            payload["cost_usd"]
        );
    }

    /// #533 discipline: a measured cost of exactly 0.0 is a measurement, not
    /// "unmeasured" — it must not be flattened into null.
    #[test]
    fn measured_zero_cost_is_not_unmeasured() {
        let _isolation = isolate_store_for_this_process();
        let dir = tempfile::tempdir().unwrap();
        init_test_ledger(dir.path());

        record_phase_done(dir.path(), "free", None, Some(0.0));

        let event = conductor_phase_event(dir.path())
            .expect("zero-cost phase must write a structured event");
        let payload = &event.payload["conductor_phase"];
        assert_eq!(
            payload["cost_usd"],
            serde_json::json!(0.0),
            "measured 0.0 must stay 0.0, not collapse into unmeasured null"
        );
    }

    /// Phase failures share the structured payload shape (GH-584: the
    /// `record_phase_failed` path had all three defects too).
    #[test]
    fn phase_failed_writes_failed_status_with_null_cost() {
        let _isolation = isolate_store_for_this_process();
        let dir = tempfile::tempdir().unwrap();
        init_test_ledger(dir.path());

        record_phase_failed(dir.path(), "verify", "check engine exploded");

        let event = conductor_phase_event(dir.path())
            .expect("phase failure must write a structured conductor_phase payload");
        let payload = &event.payload["conductor_phase"];
        assert_eq!(payload["phase_id"], "verify");
        assert_eq!(payload["status"], "failed");
        assert!(payload["cost_usd"].is_null());
    }

    /// GH-584 problem 3: plain notes must be written through the
    /// `edda_ledger` library. The pre-fix path spawned a PATH `edda` binary,
    /// which stamps role `user` (or, on CI, writes nothing at all).
    #[test]
    fn notes_write_through_the_library_with_the_agent_role() {
        let _isolation = isolate_store_for_this_process();
        let dir = tempfile::tempdir().unwrap();
        init_test_ledger(dir.path());

        record_note(dir.path(), "gate approved", &["conductor", "verdict"]);

        let events = note_events(dir.path());
        let event = events
            .iter()
            .find(|e| e.payload["text"] == "gate approved")
            .expect("record_note must append a note event to the workspace ledger");
        assert_eq!(
            event.payload["role"], "agent",
            "the conductor writes as an agent, not as the user"
        );
    }

    /// GH-584 problem 2 (guard): a missing workspace must not panic — the
    /// wrapper stays best-effort. The failure report itself is asserted by
    /// `append_ledger_note_...` error tests below.
    #[test]
    fn phase_done_on_a_workspaceless_directory_does_not_panic() {
        let dir = tempfile::tempdir().unwrap();
        record_phase_done(dir.path(), "ghost", None, None);
        record_phase_failed(dir.path(), "ghost", "boom");
        record_note(dir.path(), "ghost note", &["conductor"]);
    }

    /// GH-584 problem 2: the write path itself must surface failure — an
    /// uninitialized workspace yields a contextual error which the
    /// best-effort wrappers report on stderr, instead of the old
    /// `let _ = cmd.status()` silent swallow.
    #[test]
    fn ledger_write_fails_loudly_on_an_uninitialized_workspace() {
        let dir = tempfile::tempdir().unwrap();
        let err = append_ledger_note(dir.path(), "x", &["conductor".to_string()], None)
            .expect_err("a missing .edda workspace must be an error, not a silent no-op");
        let msg = format!("{err:#}");
        assert!(
            msg.contains("workspace ledger"),
            "error must name the failing operation, got: {msg}"
        );
    }

    /// GH-584 round-2 P1-5: parity with `edda note` — writing through the
    /// library must still refresh the derived markdown views. Without this,
    /// an operator reading `.edda/views/<branch>/log.md` right after a
    /// conductor gate/verdict/phase note silently sees stale content.
    #[test]
    fn record_note_refreshes_the_derived_log_view() {
        let dir = tempfile::tempdir().unwrap();
        edda_ledger::Ledger::ensure_initialized(dir.path()).expect("init workspace ledger");

        record_note(
            dir.path(),
            "gate approved politely",
            &["conductor", "verdict"],
        );

        let ledger = edda_ledger::Ledger::open(dir.path()).expect("open ledger");
        let branch = ledger.head_branch().expect("head branch");
        let log = std::fs::read_to_string(ledger.paths.branches_dir.join(&branch).join("log.md"))
            .expect("derived log.md must exist after a conductor note");
        assert!(
            log.contains("gate approved politely"),
            "derived log view must contain the fresh note, got:\n{log}"
        );
    }
    /// Plan terminal states get the same structured treatment; the honest
    /// total cost follows #533 (null = unmeasured).
    #[test]
    fn plan_completed_writes_structured_total_cost() {
        let _isolation = isolate_store_for_this_process();
        let dir = tempfile::tempdir().unwrap();
        init_test_ledger(dir.path());

        record_plan_completed(dir.path(), "my-plan", Some(1.5));

        let events = note_events(dir.path());
        let event = events
            .iter()
            .find(|e| e.payload.get("conductor_plan").is_some())
            .expect("plan completion must write a structured conductor_plan payload");
        let payload = &event.payload["conductor_plan"];
        assert_eq!(payload["plan_id"], "my-plan");
        assert_eq!(payload["status"], "completed");
        assert_eq!(payload["total_cost_usd"], serde_json::json!(1.5));
    }

    #[test]
    fn plan_completed_unmeasured_total_is_null() {
        let _isolation = isolate_store_for_this_process();
        let dir = tempfile::tempdir().unwrap();
        init_test_ledger(dir.path());

        record_plan_completed(dir.path(), "my-plan", None);

        let events = note_events(dir.path());
        let event = events
            .iter()
            .find(|e| e.payload.get("conductor_plan").is_some())
            .expect("unmeasured plan must still write a structured event");
        assert!(event.payload["conductor_plan"]["total_cost_usd"].is_null());
    }

    #[test]
    fn plan_aborted_writes_structured_counts() {
        let _isolation = isolate_store_for_this_process();
        let dir = tempfile::tempdir().unwrap();
        init_test_ledger(dir.path());

        record_plan_aborted(dir.path(), "my-plan", 2, 3);

        let events = note_events(dir.path());
        let event = events
            .iter()
            .find(|e| e.payload.get("conductor_plan").is_some())
            .expect("plan abort must write a structured conductor_plan payload");
        let payload = &event.payload["conductor_plan"];
        assert_eq!(payload["plan_id"], "my-plan");
        assert_eq!(payload["status"], "aborted");
        assert_eq!(payload["phases_passed"], 2);
        assert_eq!(payload["phases_pending"], 3);
    }
    #[test]
    fn ledger_phase_receipts_keep_elapsed_beside_cost_and_mark_unknown() {
        for failed in [false, true] {
            for elapsed in [None, Some(5000), Some(0)] {
                let dir = tempfile::tempdir().unwrap();
                init_test_ledger(dir.path());
                if failed {
                    record_phase_failed_timed(
                        dir.path(),
                        Some("p"),
                        "a",
                        Some(1.25),
                        "fixture",
                        elapsed,
                    );
                } else {
                    record_phase_done_timed(dir.path(), Some("p"), "a", None, Some(1.25), elapsed);
                }
                let event = conductor_phase_event(dir.path()).unwrap();
                let payload = &event.payload["conductor_phase"];
                assert_eq!(payload["elapsed_ms"].as_u64(), elapsed);
                assert_eq!(payload["elapsed_measured"], elapsed.is_some());
                assert_eq!(payload["cost_usd"], 1.25);
                if elapsed.is_none() {
                    assert!(event.payload["text"].as_str().unwrap().contains("—"));
                }
            }
        }
    }
}
