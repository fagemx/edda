//! Tmux session and pane management for multi-agent visibility.
//!
//! Creates a tmux session with one pane per phase (showing `tail -f` of
//! transcript files) plus a dashboard pane running `edda watch`.
//! All tmux interaction is via `std::process::Command` — no crate dependency.

use anyhow::{bail, Result};
use std::path::Path;
use std::process::Command;

/// Information about a single tmux pane.
#[derive(Debug, Clone)]
pub struct PaneInfo {
    pub phase_id: String,
    pub pane_id: String,
}

/// A tmux session managing panes for a conductor plan.
#[derive(Debug)]
pub struct TmuxSession {
    pub session_name: String,
    pub panes: Vec<PaneInfo>,
    pub dashboard_pane: Option<String>,
}

impl TmuxSession {
    /// Check if `tmux` is available on this system.
    pub fn is_available() -> bool {
        Command::new("tmux")
            .arg("-V")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }

    /// Derive a tmux session name from a plan name.
    /// Replaces characters that tmux disallows in session names.
    pub fn session_name_for(plan_name: &str) -> String {
        let sanitized: String = plan_name
            .chars()
            .map(|c| {
                if c.is_alphanumeric() || c == '-' || c == '_' {
                    c
                } else {
                    '-'
                }
            })
            .collect();
        format!("edda-{sanitized}")
    }

    /// Create a new tmux session with panes for each phase + dashboard.
    ///
    /// Each phase pane runs `tail -f <transcript_dir>/<phase_id>*.jsonl` so
    /// the human can see agent activity in real time. The bottom pane runs
    /// `edda watch` for the coordination dashboard.
    pub fn create(
        plan_name: &str,
        phase_ids: &[String],
        transcript_dir: &Path,
    ) -> Result<Self> {
        if !Self::is_available() {
            bail!("tmux is not installed or not in PATH");
        }

        let session_name = Self::session_name_for(plan_name);

        // Kill stale session with the same name (best-effort)
        let _ = Command::new("tmux")
            .args(["kill-session", "-t", &session_name])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();

        // Create session with the first phase pane
        let first_cmd = Self::tail_command(transcript_dir, &phase_ids[0]);
        run_tmux(&[
            "new-session",
            "-d",
            "-s",
            &session_name,
            "-n",
            "conductor",
            &first_cmd,
        ])?;

        // Set pane border format for status display
        let _ = run_tmux(&[
            "set-option",
            "-t",
            &session_name,
            "pane-border-format",
            " #{pane_title} ",
        ]);
        let _ = run_tmux(&[
            "set-option",
            "-t",
            &session_name,
            "pane-border-status",
            "top",
        ]);

        // Title the first pane
        let _ = run_tmux(&[
            "select-pane",
            "-t",
            &format!("{session_name}:0.0"),
            "-T",
            &format!("[{}] Pending", phase_ids[0]),
        ]);

        let mut panes = vec![PaneInfo {
            phase_id: phase_ids[0].clone(),
            pane_id: format!("{session_name}:0.0"),
        }];

        // Create additional phase panes
        for (i, phase_id) in phase_ids.iter().enumerate().skip(1) {
            let cmd = Self::tail_command(transcript_dir, phase_id);
            run_tmux(&[
                "split-window",
                "-t",
                &session_name,
                "-h",
                &cmd,
            ])?;

            let pane_id = format!("{session_name}:0.{i}");
            let _ = run_tmux(&[
                "select-pane",
                "-t",
                &pane_id,
                "-T",
                &format!("[{phase_id}] Pending"),
            ]);

            panes.push(PaneInfo {
                phase_id: phase_id.clone(),
                pane_id,
            });
        }

        // Dashboard pane: `edda watch` (or a placeholder if not available)
        let dashboard_cmd =
            "edda watch 2>/dev/null || { echo 'edda watch not available'; sleep infinity; }";
        let dashboard_idx = phase_ids.len();
        run_tmux(&[
            "split-window",
            "-t",
            &session_name,
            "-v",
            "-l",
            "30%",
            "-f",
            dashboard_cmd,
        ])?;
        let dashboard_pane = format!("{session_name}:0.{dashboard_idx}");
        let _ = run_tmux(&[
            "select-pane",
            "-t",
            &dashboard_pane,
            "-T",
            "Dashboard (edda watch)",
        ]);

        // Auto-tile the layout
        let _ = run_tmux(&["select-layout", "-t", &session_name, "tiled"]);

        Ok(Self {
            session_name,
            panes,
            dashboard_pane: Some(dashboard_pane),
        })
    }

    /// Update a phase pane's title to reflect its status.
    pub fn update_phase_status(&self, phase_id: &str, status: &str) -> Result<()> {
        if let Some(pane) = self.panes.iter().find(|p| p.phase_id == phase_id) {
            let title = format!("[{phase_id}] {status}");
            run_tmux(&["select-pane", "-t", &pane.pane_id, "-T", &title])?;
        }
        Ok(())
    }

    /// Destroy the tmux session.
    pub fn destroy(&self) -> Result<()> {
        run_tmux(&["kill-session", "-t", &self.session_name])?;
        Ok(())
    }

    /// Print the planned tmux layout without creating it (for --dry-run).
    pub fn print_layout_preview(plan_name: &str, phase_ids: &[String]) {
        let session_name = Self::session_name_for(plan_name);
        println!("\n  Tmux layout (session: {session_name}):");
        for (i, id) in phase_ids.iter().enumerate() {
            println!("    Pane {i}: [{id}] tail -f transcript");
        }
        println!("    Pane {}: Dashboard (edda watch)", phase_ids.len());
        println!("    Layout: tiled");
        println!("\n    Attach: tmux attach -t {session_name}");
    }

    /// Build the shell command to tail a phase transcript.
    fn tail_command(transcript_dir: &Path, phase_id: &str) -> String {
        let dir = transcript_dir.display();
        // Wait for transcript file to appear, then tail it
        format!(
            "echo 'Waiting for phase {phase_id}...' && \
             while ! ls \"{dir}/{phase_id}\"*.jsonl >/dev/null 2>&1; do sleep 1; done && \
             tail -f \"{dir}/{phase_id}\"*.jsonl"
        )
    }
}

/// Run a tmux command, returning an error if it fails.
fn run_tmux(args: &[&str]) -> Result<()> {
    let output = Command::new("tmux")
        .args(args)
        .output()
        .map_err(|e| anyhow::anyhow!("failed to run tmux: {e}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!(
            "tmux {} failed: {}",
            args.first().unwrap_or(&""),
            stderr.trim()
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_name_sanitization() {
        assert_eq!(TmuxSession::session_name_for("my-plan"), "edda-my-plan");
        assert_eq!(
            TmuxSession::session_name_for("plan with spaces"),
            "edda-plan-with-spaces"
        );
        assert_eq!(TmuxSession::session_name_for("plan.v2"), "edda-plan-v2");
        assert_eq!(
            TmuxSession::session_name_for("under_score"),
            "edda-under_score"
        );
    }

    #[test]
    fn tail_command_format() {
        let dir = std::path::PathBuf::from("/tmp/transcripts");
        let cmd = TmuxSession::tail_command(&dir, "build");
        assert!(cmd.contains("phase build"));
        assert!(cmd.contains("tail -f"));
        assert!(cmd.contains("/tmp/transcripts/build"));
    }

    #[test]
    fn layout_preview_does_not_panic() {
        let phases = vec!["a".to_string(), "b".to_string(), "c".to_string()];
        TmuxSession::print_layout_preview("test-plan", &phases);
    }
}
