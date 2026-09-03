use std::io::Read;
use std::path::Path;

// ── Command Implementations ──

/// `edda bridge claude install`
pub fn install(repo_root: &Path, no_claude_md: bool) -> anyhow::Result<()> {
    edda_bridge_claude::install(repo_root, no_claude_md)
}

/// `edda bridge claude uninstall`
pub fn uninstall(repo_root: &Path) -> anyhow::Result<()> {
    edda_bridge_claude::uninstall(repo_root)
}

/// `edda hook claude` — read stdin, dispatch hook
///
/// Resilience: catch_unwind + configurable timeout (EDDA_HOOK_TIMEOUT_MS).
/// On panic or timeout, exits 0 — never blocks the host agent.
pub fn hook_claude() -> anyhow::Result<()> {
    run_hook_resilient("", |stdin| {
        let r = edda_bridge_claude::hook_entrypoint_from_stdin(&stdin)?;
        Ok((r.stdout, r.stderr))
    })
}

/// Shared resilience wrapper: read stdin, spawn worker with catch_unwind + timeout.
///
/// `prefix` is prepended to debug log messages (e.g., `""` for Claude, `"OPENCLAW "` for OpenClaw).
/// `entrypoint` receives the stdin string and returns (stdout, stderr).
pub(super) fn run_hook_resilient<F>(prefix: &str, entrypoint: F) -> anyhow::Result<()>
where
    F: FnOnce(String) -> anyhow::Result<(Option<String>, Option<String>)> + Send + 'static,
{
    let mut stdin_buf = String::new();
    if let Err(e) = std::io::stdin().read_to_string(&mut stdin_buf) {
        debug_log(&format!("{prefix}STDIN READ ERROR: {e}"));
        return Ok(());
    }

    debug_log(&format!(
        "{prefix}STDIN({} bytes): {}",
        stdin_buf.len(),
        &stdin_buf[..stdin_buf.len().min(200)]
    ));

    let timeout_ms = hook_timeout_ms();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let result =
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| entrypoint(stdin_buf)));
        let _ = tx.send(result);
    });

    match rx.recv_timeout(std::time::Duration::from_millis(timeout_ms)) {
        Ok(Ok(Ok((stdout, stderr)))) => {
            if let Some(output) = &stdout {
                debug_log(&format!("{prefix}OK output({} bytes)", output.len()));
                print!("{output}");
            }
            if let Some(warning) = &stderr {
                debug_log(&format!("{prefix}WARNING: {warning}"));
                eprintln!("{warning}");
                // Exit 1 = non-blocking warning; Claude Code shows stderr to user
                // but does not feed it to the model or block the conversation.
                std::process::exit(1);
            }
            if stdout.is_none() && stderr.is_none() {
                debug_log(&format!("{prefix}OK (no output)"));
            }
            Ok(())
        }
        Ok(Ok(Err(e))) => {
            debug_log(&format!("{prefix}ERROR: {e}"));
            Ok(())
        }
        Ok(Err(panic_info)) => {
            let msg = panic_info
                .downcast_ref::<String>()
                .map(|s| s.as_str())
                .or_else(|| panic_info.downcast_ref::<&str>().copied())
                .unwrap_or("unknown panic");
            debug_log(&format!("{prefix}PANIC: {msg}"));
            Ok(())
        }
        Err(_) => {
            debug_log(&format!(
                "{prefix}TIMEOUT after {timeout_ms}ms — graceful exit"
            ));
            Ok(())
        }
    }
}

/// Hook timeout in milliseconds. Configurable via `EDDA_HOOK_TIMEOUT_MS` (default: 60s).
///
/// Raised from 10s to 60s to accommodate SessionEnd background threads that
/// make LLM API calls (bg_extract, bg_digest, bg_scan, bg_detect).  See #287.
pub(super) fn hook_timeout_ms() -> u64 {
    std::env::var("EDDA_HOOK_TIMEOUT_MS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(60_000)
}

fn debug_log(msg: &str) {
    if std::env::var_os("EDDA_DEBUG").is_none() {
        return;
    }
    use std::io::Write;
    let log_path = std::env::temp_dir().join("edda-hook-debug.log");
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
    {
        let ts = time::OffsetDateTime::now_utc()
            .format(&time::format_description::well_known::Rfc3339)
            .unwrap_or_default();
        // swallow-ok: optional EDDA_DEBUG log must never fail the hook path
        let _ = writeln!(f, "[{ts}] {msg}");
    }
}

/// `edda doctor claude`
pub fn doctor(repo_root: &Path) -> anyhow::Result<()> {
    edda_bridge_claude::doctor(repo_root)
}

pub fn digest(repo_root: &Path, session: Option<&str>, all: bool) -> anyhow::Result<()> {
    let project_id = edda_store::project_id(repo_root);
    let cwd = repo_root.to_str().unwrap_or(".");

    if let Some(session_id) = session {
        println!("Digesting session {session_id}...");
        let event_id =
            edda_bridge_claude::digest::digest_session_manual(&project_id, session_id, cwd, true)?;
        println!("  Written: {event_id}");
        return Ok(());
    }

    if all {
        let pending = edda_bridge_claude::digest::find_all_pending_sessions(&project_id);
        if pending.is_empty() {
            println!("No pending sessions to digest.");
            return Ok(());
        }
        println!("Found {} pending sessions", pending.len());
        for session_id in &pending {
            print!("  Digesting {session_id}...");
            match edda_bridge_claude::digest::digest_session_manual(
                &project_id,
                session_id,
                cwd,
                true,
            ) {
                Ok(event_id) => println!(" OK ({event_id})"),
                Err(e) => println!(" FAILED: {e}"),
            }
        }
        return Ok(());
    }

    anyhow::bail!("must specify --session <id> or --all")
}

/// `edda index verify --project <id> --session <id> [--sample N] [--all]`
pub fn index_verify(
    project_id: &str,
    session_id: &str,
    sample: usize,
    all: bool,
) -> anyhow::Result<()> {
    let project_dir = edda_store::project_dir(project_id);
    let index_path = project_dir
        .join("index")
        .join(format!("{session_id}.jsonl"));
    let store_path = project_dir
        .join("transcripts")
        .join(format!("{session_id}.jsonl"));

    if !index_path.exists() {
        anyhow::bail!("index file not found: {}", index_path.display());
    }
    if !store_path.exists() {
        anyhow::bail!("store file not found: {}", store_path.display());
    }

    let max_lines = if all { usize::MAX } else { sample * 2 };
    let records = edda_index::read_index_tail(&index_path, max_lines, 64 * 1024 * 1024)?;

    let check_count = if all {
        records.len()
    } else {
        sample.min(records.len())
    };

    // Sample evenly from the records
    let step = if check_count == 0 {
        1
    } else {
        (records.len() as f64 / check_count as f64).ceil() as usize
    };

    let mut checked = 0;
    let mut mismatches = 0;

    for (i, rec) in records.iter().enumerate() {
        if !all && i % step != 0 && checked >= check_count {
            continue;
        }
        if checked >= check_count {
            break;
        }

        let fetched = edda_index::fetch_store_line(&store_path, rec.store_offset, rec.store_len)?;
        let parsed: serde_json::Value = serde_json::from_slice(&fetched)?;
        let fetched_uuid = parsed.get("uuid").and_then(|v| v.as_str()).unwrap_or("");

        if fetched_uuid != rec.uuid {
            println!(
                "MISMATCH at index record {}: expected uuid={}, got uuid={}",
                i, rec.uuid, fetched_uuid
            );
            mismatches += 1;
        }
        checked += 1;
    }

    if mismatches > 0 {
        anyhow::bail!("{mismatches} mismatches found in {checked} checks");
    }

    println!("OK: {checked} index records verified, 0 mismatches");
    Ok(())
}
