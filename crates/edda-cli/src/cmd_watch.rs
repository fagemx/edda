use std::path::Path;

/// Launch the real-time watch view.
///
/// With the `tui` feature (default): opens the interactive ratatui TUI.
/// Without: prints a plain-text event stream to stdout.
pub fn execute(repo_root: &Path) -> anyhow::Result<()> {
    let project_id = edda_store::project_id(repo_root);

    #[cfg(feature = "tui")]
    {
        crate::tui::run(project_id, repo_root.to_path_buf())
    }

    #[cfg(not(feature = "tui"))]
    {
        use edda_bridge_claude::watch;

        // Auto-init
        if let Err(e) = edda_store::ensure_dirs(&project_id) {
            eprintln!("Warning: failed to ensure store dirs: {e}");
        }
        if let Err(e) = edda_ledger::Ledger::ensure_initialized(repo_root) {
            eprintln!("Warning: failed to auto-init .edda/: {e}");
        }

        eprintln!("edda watch (plain mode — rebuild with `tui` feature for interactive UI)");
        eprintln!("Press Ctrl-C to stop.\n");

        let mut last_count = 0usize;
        loop {
            match watch::snapshot(&project_id, repo_root, 200) {
                Ok(data) => {
                    for evt in data.events.iter().skip(last_count) {
                        let ts = if evt.ts.len() >= 19 {
                            &evt.ts[11..19]
                        } else {
                            &evt.ts
                        };
                        let preview = evt
                            .payload
                            .get("text")
                            .and_then(|v| v.as_str())
                            .or_else(|| evt.payload.get("message").and_then(|v| v.as_str()))
                            .unwrap_or("");
                        let first = preview.lines().next().unwrap_or(preview);
                        println!("{ts}  {:<10} {first}", evt.event_type);
                    }
                    last_count = data.events.len();
                }
                Err(e) => {
                    eprintln!("error: {e}");
                }
            }
            std::thread::sleep(std::time::Duration::from_secs(2));
        }
    }
}
