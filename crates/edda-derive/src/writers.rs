use anyhow::Result;
use edda_ledger::Ledger;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::path::Path;

use crate::snapshot::{
    as_arr_str, as_str, build_branch_snapshot, collect_branch_events, fmt_cmd_argv,
};
use crate::types::*;

// ── View writers ──

fn ensure_branch_dir(ledger: &Ledger, branch: &str) -> Result<std::path::PathBuf> {
    let dir = ledger.paths.branch_dir(branch);
    fs::create_dir_all(&dir)?;
    Ok(dir)
}

fn write_commit_md(dir: &Path, snap: &BranchSnapshot) -> Result<()> {
    let mut out = String::new();
    for c in &snap.commits {
        out.push_str(&format!("## {} {} — {}\n\n", c.ts, c.event_id, c.title));
        out.push_str(&format!(
            "- Purpose: {}\n",
            if c.purpose.is_empty() {
                "(empty)"
            } else {
                &c.purpose
            }
        ));
        out.push_str(&format!(
            "- Previous Progress Summary: {}\n",
            if c.prev_summary.is_empty() {
                "(empty)"
            } else {
                &c.prev_summary
            }
        ));
        out.push_str(&format!(
            "- This Commit's Contribution: {}\n",
            if c.contribution.is_empty() {
                "(empty)"
            } else {
                &c.contribution
            }
        ));
        out.push_str("- Evidence:\n");
        if c.evidence_lines.is_empty() {
            out.push_str("  - (none)\n");
        } else {
            for e in &c.evidence_lines {
                out.push_str(&format!("  - {e}\n"));
            }
        }
        out.push_str("- Labels: ");
        if c.labels.is_empty() {
            out.push_str("(none)\n\n");
        } else {
            out.push_str(&c.labels.join(", "));
            out.push_str("\n\n");
        }
    }
    fs::write(dir.join("commit.md"), out.as_bytes())?;
    Ok(())
}

fn write_log_md(dir: &Path, ledger: &Ledger, branch: &str) -> Result<()> {
    let branch_events = collect_branch_events(ledger, branch)?;
    let mut out = String::new();

    for ev in &branch_events {
        match ev.event_type.as_str() {
            "note" => {
                let role = ev
                    .payload
                    .get("role")
                    .and_then(|x| x.as_str())
                    .unwrap_or("user");
                let text = ev
                    .payload
                    .get("text")
                    .and_then(|x| x.as_str())
                    .unwrap_or("");
                let tags: String = ev
                    .payload
                    .get("tags")
                    .and_then(|x| x.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|i| i.as_str())
                            .collect::<Vec<_>>()
                            .join(",")
                    })
                    .unwrap_or_default();

                if tags.is_empty() {
                    out.push_str(&format!(
                        "[{}] NOTE({}): {} ({})\n",
                        ev.ts, role, text, ev.event_id
                    ));
                } else {
                    out.push_str(&format!(
                        "[{}] NOTE({}) tags={}: {} ({})\n",
                        ev.ts, role, tags, text, ev.event_id
                    ));
                }
            }
            "cmd" => {
                let exit_code = ev
                    .payload
                    .get("exit_code")
                    .and_then(|x| x.as_i64())
                    .unwrap_or(0);
                let argv = fmt_cmd_argv(&ev.payload);
                let stdout_blob = ev
                    .payload
                    .get("stdout_blob")
                    .and_then(|x| x.as_str())
                    .unwrap_or("");
                let stderr_blob = ev
                    .payload
                    .get("stderr_blob")
                    .and_then(|x| x.as_str())
                    .unwrap_or("");
                out.push_str(&format!(
                    "[{}] CMD exit={}: {} (stdout={}, stderr={}) ({})\n",
                    ev.ts, exit_code, argv, stdout_blob, stderr_blob, ev.event_id
                ));
            }
            "commit" => {
                let title = ev
                    .payload
                    .get("title")
                    .and_then(|x| x.as_str())
                    .unwrap_or("");
                out.push_str(&format!(
                    "[{}] COMMIT: {} ({})\n",
                    ev.ts, title, ev.event_id
                ));
            }
            "rebuild" => {
                let scope = ev
                    .payload
                    .get("scope")
                    .and_then(|x| x.as_str())
                    .unwrap_or("");
                let reason = ev
                    .payload
                    .get("reason")
                    .and_then(|x| x.as_str())
                    .unwrap_or("");
                out.push_str(&format!(
                    "[{}] REBUILD scope={} reason={} ({})\n",
                    ev.ts, scope, reason, ev.event_id
                ));
            }
            "branch_create" => {
                let name = as_str(&ev.payload, "name");
                let purpose = as_str(&ev.payload, "purpose");
                out.push_str(&format!(
                    "[{}] BRANCH_CREATE: {} purpose=\"{}\" ({})\n",
                    ev.ts, name, purpose, ev.event_id
                ));
            }
            "branch_switch" => {
                let from = as_str(&ev.payload, "from");
                let to = as_str(&ev.payload, "to");
                out.push_str(&format!(
                    "[{}] SWITCH: {} -> {} ({})\n",
                    ev.ts, from, to, ev.event_id
                ));
            }
            "merge" => {
                let src = as_str(&ev.payload, "src");
                let dst = as_str(&ev.payload, "dst");
                let reason = as_str(&ev.payload, "reason");
                let adopted = ev
                    .payload
                    .get("adopted_commits")
                    .and_then(|x| x.as_array())
                    .map(|a| a.len())
                    .unwrap_or(0);
                out.push_str(&format!(
                    "[{}] MERGE: {} -> {} adopted={} reason=\"{}\" ({})\n",
                    ev.ts, src, dst, adopted, reason, ev.event_id
                ));
            }
            "approval" => {
                let draft_id = as_str(&ev.payload, "draft_id");
                let decision = as_str(&ev.payload, "decision");
                let actor = as_str(&ev.payload, "actor");
                let stage_id = as_str(&ev.payload, "stage_id");
                let role = as_str(&ev.payload, "role");
                if stage_id.is_empty() {
                    out.push_str(&format!(
                        "[{}] APPROVAL {} by={} draft={} ({})\n",
                        ev.ts, decision, actor, draft_id, ev.event_id
                    ));
                } else {
                    out.push_str(&format!(
                        "[{}] APPROVAL {} by={} draft={} stage={} role={} ({})\n",
                        ev.ts, decision, actor, draft_id, stage_id, role, ev.event_id
                    ));
                }
            }
            "approval_request" => {
                let draft_id = as_str(&ev.payload, "draft_id");
                let stage_id = as_str(&ev.payload, "stage_id");
                let role = as_str(&ev.payload, "role");
                let assignees = as_arr_str(&ev.payload, "assignees");
                out.push_str(&format!(
                    "[{}] APPROVAL_REQUEST draft={} stage={} role={} assignees={} ({})\n",
                    ev.ts,
                    draft_id,
                    stage_id,
                    role,
                    assignees.join(","),
                    ev.event_id
                ));
            }
            other => {
                out.push_str(&format!(
                    "[{}] {} ({})\n",
                    ev.ts,
                    other.to_uppercase(),
                    ev.event_id
                ));
            }
        }
    }
    fs::write(dir.join("log.md"), out.as_bytes())?;
    Ok(())
}

#[derive(Debug, Serialize, Deserialize)]
struct MetadataYaml {
    repo_root: String,
    created_at: String,
    head: String,
    branch: String,
    last_event_id: String,
    last_commit_id: String,
}

fn write_metadata_yaml(dir: &Path, ledger: &Ledger, snap: &BranchSnapshot) -> Result<()> {
    let head = ledger.head_branch().unwrap_or_else(|_| "main".to_string());

    let m = MetadataYaml {
        repo_root: ".".to_string(),
        created_at: snap.created_at.clone(),
        head,
        branch: snap.branch.clone(),
        last_event_id: snap.last_event_id.clone().unwrap_or_default(),
        last_commit_id: snap.last_commit_id.clone().unwrap_or_default(),
    };

    let yaml = serde_yaml::to_string(&m)?;
    fs::write(dir.join("metadata.yaml"), yaml.as_bytes())?;
    Ok(())
}

fn write_main_md(dir: &Path, ledger: &Ledger, snap: &BranchSnapshot) -> Result<()> {
    let head = ledger.head_branch().unwrap_or_else(|_| "main".to_string());

    let mut out = String::new();
    out.push_str("# MAIN\n\n");
    out.push_str(&format!("- head: {head}\n"));
    out.push_str(&format!("- branch: {}\n", snap.branch));
    out.push_str(&format!(
        "- uncommitted_events: {}\n",
        snap.uncommitted_events
    ));

    if let Some(c) = &snap.last_commit {
        out.push_str(&format!(
            "- last_commit: {} {} \"{}\"\n",
            c.ts, c.event_id, c.title
        ));
    } else {
        out.push_str("- last_commit: (none)\n");
    }

    if let Some(m) = snap.merges.last() {
        out.push_str(&format!(
            "- last_merge: {} {} {}->{} adopted={}\n",
            m.ts,
            m.event_id,
            m.src,
            m.dst,
            m.adopted_commits.len()
        ));
    } else {
        out.push_str("- last_merge: (none)\n");
    }

    fs::write(dir.join("main.md"), out.as_bytes())?;
    Ok(())
}

fn write_branches_json(ledger: &Ledger, snaps: &[BranchSnapshot]) -> Result<()> {
    let mut branches: BTreeMap<String, Value> = BTreeMap::new();
    for s in snaps {
        branches.insert(
            s.branch.clone(),
            serde_json::json!({
                "created_at": s.created_at,
                "last_event_id": s.last_event_id.clone().unwrap_or_default(),
                "last_commit_id": s.last_commit_id.clone().unwrap_or_default()
            }),
        );
    }
    let root = serde_json::json!({ "branches": branches });
    let text = serde_json::to_string_pretty(&root)?;
    fs::write(&ledger.paths.branches_json, text.as_bytes())?;
    Ok(())
}

fn list_branches_from_ledger(ledger: &Ledger) -> Result<Vec<String>> {
    let mut set: HashSet<String> = HashSet::new();
    set.insert("main".to_string());
    for ev in ledger.iter_events()? {
        if !ev.branch.trim().is_empty() {
            set.insert(ev.branch.clone());
        }
        // Also pick up branch names from branch_create payload
        if ev.event_type == "branch_create" {
            if let Some(name) = ev.payload.get("name").and_then(|x| x.as_str()) {
                if !name.trim().is_empty() {
                    set.insert(name.to_string());
                }
            }
        }
    }
    let mut v: Vec<String> = set.into_iter().collect();
    v.sort();
    Ok(v)
}

// ── Public API ──

pub fn rebuild_branch(ledger: &Ledger, branch: &str) -> Result<BranchSnapshot> {
    let snap = build_branch_snapshot(ledger, branch)?;
    let dir = ensure_branch_dir(ledger, branch)?;
    write_commit_md(&dir, &snap)?;
    write_log_md(&dir, ledger, branch)?;
    write_metadata_yaml(&dir, ledger, &snap)?;
    write_main_md(&dir, ledger, &snap)?;
    Ok(snap)
}

pub fn rebuild_all(ledger: &Ledger) -> Result<Vec<BranchSnapshot>> {
    let branches = list_branches_from_ledger(ledger)?;
    let mut snaps: Vec<BranchSnapshot> = Vec::new();
    for b in &branches {
        snaps.push(rebuild_branch(ledger, b)?);
    }
    write_branches_json(ledger, &snaps)?;
    Ok(snaps)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::setup_workspace;
    use edda_core::event::{new_commit_event, new_merge_event, new_note_event, CommitEventParams};

    #[test]
    fn rebuild_branch_creates_view_files() {
        let (tmp, ledger) = setup_workspace();

        // Add a note and a commit
        let note = new_note_event("main", None, "user", "test note", &[]).unwrap();
        ledger.append_event(&note, false).unwrap();

        let mut params = CommitEventParams {
            branch: "main",
            parent_hash: None,
            title: "first commit",
            purpose: Some("test"),
            prev_summary: "",
            contribution: "added tests",
            evidence: vec![],
            labels: vec![],
        };
        let commit = new_commit_event(&mut params).unwrap();
        ledger.append_event(&commit, false).unwrap();

        let snap = rebuild_branch(&ledger, "main").unwrap();
        assert_eq!(snap.branch, "main");
        assert_eq!(snap.commits.len(), 1);
        assert_eq!(snap.commits[0].title, "first commit");
        assert_eq!(snap.commits[0].contribution, "added tests");

        // Verify files were created
        let branch_dir = ledger.paths.branches_dir.join("main");
        assert!(branch_dir.join("commit.md").exists());
        assert!(branch_dir.join("log.md").exists());
        assert!(branch_dir.join("metadata.yaml").exists());
        assert!(branch_dir.join("main.md").exists());

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn rebuild_all_handles_multiple_branches() {
        let (tmp, ledger) = setup_workspace();

        // Add events on main
        let note = new_note_event("main", None, "user", "main note", &[]).unwrap();
        ledger.append_event(&note, false).unwrap();

        // Create a second branch event
        let branch_evt = edda_core::event::new_branch_create_event(
            "feature",
            None,
            "feature",
            "testing feature",
            "main",
            None,
        )
        .unwrap();
        ledger.append_event(&branch_evt, false).unwrap();

        let note2 = new_note_event("feature", None, "user", "feature note", &[]).unwrap();
        ledger.append_event(&note2, false).unwrap();

        let snaps = rebuild_all(&ledger).unwrap();
        assert!(snaps.len() >= 2);

        let branch_names: Vec<&str> = snaps.iter().map(|s| s.branch.as_str()).collect();
        assert!(branch_names.contains(&"main"));
        assert!(branch_names.contains(&"feature"));

        // branches.json should exist (in refs_dir, not branches_dir)
        assert!(ledger.paths.branches_json.exists());

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn rebuild_branch_snapshot_counts_uncommitted() {
        let (tmp, ledger) = setup_workspace();

        // Add notes without any commit
        let n1 = new_note_event("main", None, "user", "note 1", &[]).unwrap();
        let n2 = new_note_event("main", None, "user", "note 2", &[]).unwrap();
        ledger.append_event(&n1, false).unwrap();
        ledger.append_event(&n2, false).unwrap();

        let snap = rebuild_branch(&ledger, "main").unwrap();
        assert_eq!(snap.commits.len(), 0);
        assert_eq!(snap.uncommitted_events, 2);

        // Now add a commit
        let mut params = CommitEventParams {
            branch: "main",
            parent_hash: None,
            title: "wrap up",
            purpose: None,
            prev_summary: "",
            contribution: "done",
            evidence: vec![],
            labels: vec![],
        };
        let c = new_commit_event(&mut params).unwrap();
        ledger.append_event(&c, false).unwrap();

        let snap2 = rebuild_branch(&ledger, "main").unwrap();
        assert_eq!(snap2.commits.len(), 1);
        assert_eq!(snap2.uncommitted_events, 0);

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn rebuild_branch_captures_merge() {
        let (tmp, ledger) = setup_workspace();

        let merge =
            new_merge_event("main", None, "feature", "main", "merge feature work", &[]).unwrap();
        ledger.append_event(&merge, false).unwrap();

        let snap = rebuild_branch(&ledger, "main").unwrap();
        assert_eq!(snap.merges.len(), 1);
        assert_eq!(snap.merges[0].src, "feature");
        assert_eq!(snap.merges[0].dst, "main");
        assert_eq!(snap.merges[0].reason, "merge feature work");

        let _ = std::fs::remove_dir_all(&tmp);
    }
}
