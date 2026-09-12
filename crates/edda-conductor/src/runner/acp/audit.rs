use super::{AcpAudit, AcpUsage};
use anyhow::{Context, Result};
use edda_core::event::{
    finalize_event, new_note_event, new_task_session_event,
    new_task_session_event_with_execution_brief, ControlledTaskSessionParams,
};
use edda_ledger::lock::{TaskDispatchLock, WorkspaceLock};
use edda_ledger::task_actions::CONTROLLED_TASK_LEASE_PREFIX;
use edda_ledger::tasks::{TaskStatus, TaskView};
use edda_ledger::Ledger;
use std::path::PathBuf;
use std::sync::Mutex;

/// Ledger-backed ACP audit. The session event is written immediately after a
/// successful `session/new`, before any prompt can make side effects.
pub struct LedgerAcpAudit {
    workspace: PathBuf,
    execution_brief: Option<ControlledAcpBinding>,
    turn_guard: Mutex<Option<TaskDispatchLock>>,
}

struct ControlledAcpBinding {
    event_id: String,
    digest: String,
    attempt: u32,
    lease_owner: String,
    agent_kind: String,
}

impl LedgerAcpAudit {
    pub fn new(workspace: impl Into<PathBuf>) -> Self {
        Self {
            workspace: workspace.into(),
            execution_brief: None,
            turn_guard: Mutex::new(None),
        }
    }

    pub fn with_execution_brief(
        mut self,
        event_id: impl Into<String>,
        digest: impl Into<String>,
        attempt: u32,
        lease_owner: impl Into<String>,
        agent_kind: impl Into<String>,
    ) -> Self {
        self.execution_brief = Some(ControlledAcpBinding {
            event_id: event_id.into(),
            digest: digest.into(),
            attempt,
            lease_owner: lease_owner.into(),
            agent_kind: agent_kind.into(),
        });
        self
    }

    pub fn with_task_dispatch_lock(self, task_id: u64, guard: TaskDispatchLock) -> Result<Self> {
        anyhow::ensure!(
            guard.task_id() == task_id,
            "ACP task dispatch guard belongs to another task"
        );
        *self
            .turn_guard
            .lock()
            .map_err(|_| anyhow::anyhow!("ACP task dispatch guard lock poisoned"))? = Some(guard);
        Ok(self)
    }

    fn ensure_turn_guard(&self, task_id: u64) -> Result<()> {
        let mut slot = self
            .turn_guard
            .lock()
            .map_err(|_| anyhow::anyhow!("ACP task dispatch guard lock poisoned"))?;
        if let Some(guard) = slot.as_ref() {
            anyhow::ensure!(
                guard.task_id() == task_id,
                "ACP task dispatch guard belongs to another task"
            );
            return Ok(());
        }
        let paths = edda_ledger::EddaPaths::discover(&self.workspace);
        *slot = Some(TaskDispatchLock::acquire(&paths, task_id)?);
        Ok(())
    }

    fn current_task(&self, ledger: &Ledger, task_id: u64, stage: &str) -> Result<TaskView> {
        let task = ledger
            .task_views()?
            .into_iter()
            .find(|view| view.task_id == task_id)
            .with_context(|| format!("ACP task disappeared before {stage}"))?;
        anyhow::ensure!(
            task.status == TaskStatus::Running,
            "ACP task is no longer running before {stage}"
        );
        if let Some(binding) = &self.execution_brief {
            anyhow::ensure!(
                task.attempts == binding.attempt,
                "ACP controlled attempt changed before {stage}"
            );
            let lease = ledger
                .task_lease(task_id)?
                .with_context(|| format!("ACP controlled lease disappeared before {stage}"))?;
            anyhow::ensure!(
                lease.attempt == binding.attempt && lease.owner == binding.lease_owner,
                "ACP controlled lease changed before {stage}"
            );
            let expires = time::OffsetDateTime::parse(
                &lease.expires_at,
                &time::format_description::well_known::Rfc3339,
            )?;
            anyhow::ensure!(
                expires > time::OffsetDateTime::now_utc(),
                "ACP controlled lease expired before {stage}"
            );
            let prefix = format!(
                "{CONTROLLED_TASK_LEASE_PREFIX}{}:{}:",
                binding.event_id, binding.digest
            );
            anyhow::ensure!(
                binding.lease_owner.starts_with(&prefix),
                "ACP controlled lease is bound to another brief"
            );
        }
        Ok(task)
    }

    fn append_note(&self, task_id: u64, kind: &'static str, allowed: bool) -> Result<()> {
        let ledger = Ledger::open(&self.workspace).context("opening ACP task ledger")?;
        let _lock = WorkspaceLock::acquire(&ledger.paths).context("locking ACP task ledger")?;
        if kind == "session_load" {
            let task = self.current_task(&ledger, task_id, "session resume")?;
            if let Some(binding) = &self.execution_brief {
                anyhow::ensure!(
                    task.session_attempt == Some(binding.attempt)
                        && task.session_lease_owner.as_deref()
                            == Some(binding.lease_owner.as_str())
                        && task.session_brief_event_id.as_deref()
                            == Some(binding.event_id.as_str())
                        && task.session_brief_digest.as_deref() == Some(binding.digest.as_str()),
                    "ACP controlled session binding changed before resume"
                );
            }
        }
        let branch = ledger.head_branch().context("reading ACP ledger branch")?;
        let parent = ledger
            .last_event_hash()
            .context("reading ACP ledger head")?;
        let tags = vec!["acp".to_string(), kind.to_string()];
        let mut event = new_note_event(
            &branch,
            parent.as_deref(),
            "agent",
            "ACP policy decision",
            &tags,
        )?;
        event.payload["acp"] = serde_json::json!({
            "task_id": task_id,
            "kind": kind,
            "allowed": allowed,
        });
        finalize_event(&mut event)?;
        ledger
            .append_event(&event)
            .context("appending ACP audit event")?;
        Ok(())
    }
}

impl AcpAudit for LedgerAcpAudit {
    fn session_created(&self, task_id: u64, session_id: &str) -> Result<()> {
        let ledger = Ledger::open(&self.workspace).context("opening ACP task ledger")?;
        let _lock = WorkspaceLock::acquire(&ledger.paths).context("locking ACP task ledger")?;
        let task = self.current_task(&ledger, task_id, "session persistence")?;
        let branch = ledger.head_branch().context("reading ACP ledger branch")?;
        let parent = ledger
            .last_event_hash()
            .context("reading ACP ledger head")?;
        let event = match &self.execution_brief {
            Some(binding) => new_task_session_event_with_execution_brief(
                &branch,
                parent.as_deref(),
                task_id,
                &ControlledTaskSessionParams {
                    agent_kind: &binding.agent_kind,
                    session_id,
                    attempt: binding.attempt,
                    lease_owner: &binding.lease_owner,
                    brief_event_id: &binding.event_id,
                    brief_digest: &binding.digest,
                },
            )?,
            None => new_task_session_event(&branch, parent.as_deref(), task_id, session_id)?,
        };
        // The task was read under the same workspace lock as this append.
        // Keep this explicit so a future refactor does not drop the check.
        anyhow::ensure!(task.status == TaskStatus::Running, "ACP task changed");
        ledger
            .append_event(&event)
            .context("appending task.session")?;
        Ok(())
    }

    fn session_ready(&self, task_id: u64, session_id: &str) -> Result<()> {
        // Acquire/retain the same generation lock used by direct public
        // task.started/task.failed appends before the final prompt check. It
        // remains held for this audit sink's lifetime, including permission
        // callbacks during the turn.
        self.ensure_turn_guard(task_id)?;
        let ledger = Ledger::open(&self.workspace).context("opening ACP task ledger")?;
        let _lock = WorkspaceLock::acquire(&ledger.paths).context("locking ACP task ledger")?;
        let task = self.current_task(&ledger, task_id, "prompt")?;
        anyhow::ensure!(
            task.acp_session_id.as_deref() == Some(session_id),
            "ACP task or session changed before prompt"
        );
        match &self.execution_brief {
            Some(binding) => anyhow::ensure!(
                task.session_attempt == Some(binding.attempt)
                    && task.session_lease_owner.as_deref() == Some(binding.lease_owner.as_str())
                    && task.session_agent_kind.as_deref() == Some(binding.agent_kind.as_str())
                    && task.session_brief_event_id.as_deref() == Some(binding.event_id.as_str())
                    && task.session_brief_digest.as_deref() == Some(binding.digest.as_str()),
                "ACP controlled execution binding changed before prompt"
            ),
            None => anyhow::ensure!(
                task.session_brief_event_id.is_none()
                    && task.session_brief_digest.is_none()
                    && task.session_lease_owner.is_none(),
                "ACP session became controlled before legacy prompt"
            ),
        }
        Ok(())
    }

    fn decision(&self, task_id: u64, kind: &'static str, allowed: bool) -> Result<()> {
        self.append_note(task_id, kind, allowed)
    }

    fn update(&self, task_id: u64, kind: &'static str) -> Result<()> {
        self.append_note(task_id, kind, true)
    }

    fn usage(&self, task_id: u64, usage: Option<&AcpUsage>) -> Result<()> {
        let ledger = Ledger::open(&self.workspace).context("opening ACP task ledger")?;
        let _lock = WorkspaceLock::acquire(&ledger.paths).context("locking ACP task ledger")?;
        let branch = ledger.head_branch().context("reading ACP ledger branch")?;
        let parent = ledger
            .last_event_hash()
            .context("reading ACP ledger head")?;
        let mut event = new_note_event(
            &branch,
            parent.as_deref(),
            "agent",
            "ACP usage receipt",
            &["acp".into(), "usage".into()],
        )?;
        event.payload["acp"] = serde_json::json!({
            "task_id": task_id,
            "measured": usage.is_some(),
            "usage": usage,
        });
        finalize_event(&mut event)?;
        ledger
            .append_event(&event)
            .context("appending ACP usage receipt")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use edda_core::event::{
        new_task_created_event, new_task_failed_event, new_task_started_event, TaskCreatedParams,
    };
    use edda_ledger::TaskLease;

    fn setup(root: &std::path::Path) -> (Ledger, LedgerAcpAudit, String) {
        Ledger::ensure_initialized(root).unwrap();
        let ledger = Ledger::open(root).unwrap();
        ledger
            .append_event(
                &new_task_created_event(&TaskCreatedParams {
                    branch: "main",
                    parent_hash: None,
                    task_id: 1,
                    title: "controlled ACP",
                    assignee: None,
                    agent_kind: Some("acp:grok"),
                    after: &[],
                    plan_id: None,
                    work_unit_ref: None,
                    brief_ref: None,
                    idempotency_key: None,
                    scope_paths: &["src".into()],
                })
                .unwrap(),
            )
            .unwrap();
        let parent = ledger.last_event_hash().unwrap();
        ledger
            .append_event(&new_task_started_event("main", parent.as_deref(), 1, 300, 1).unwrap())
            .unwrap();
        let owner = format!(
            "{CONTROLLED_TASK_LEASE_PREFIX}evt_controlled:{}:owner",
            "a".repeat(64)
        );
        ledger
            .upsert_task_lease(&TaskLease {
                task_id: 1,
                attempt: 1,
                owner: owner.clone(),
                expires_at: "2999-01-01T00:00:00Z".into(),
                heartbeat_at: "2026-09-11T00:00:00Z".into(),
            })
            .unwrap();
        let audit = LedgerAcpAudit::new(root).with_execution_brief(
            "evt_controlled",
            "a".repeat(64),
            1,
            &owner,
            "acp:grok",
        );
        (ledger, audit, owner)
    }

    fn restart(ledger: &Ledger) {
        let parent = ledger.last_event_hash().unwrap();
        ledger
            .append_event(&new_task_failed_event("main", parent.as_deref(), 1, "restart").unwrap())
            .unwrap();
        let parent = ledger.last_event_hash().unwrap();
        ledger
            .append_event(&new_task_started_event("main", parent.as_deref(), 1, 300, 2).unwrap())
            .unwrap();
        ledger
            .upsert_task_lease(&TaskLease {
                task_id: 1,
                attempt: 2,
                owner: "replacement".into(),
                expires_at: "2999-01-01T00:00:00Z".into(),
                heartbeat_at: "2026-09-11T00:01:00Z".into(),
            })
            .unwrap();
    }

    #[test]
    fn controlled_session_persistence_refuses_fail_restart_race() {
        let root = tempfile::tempdir().unwrap();
        let (ledger, audit, _) = setup(root.path());
        restart(&ledger);

        assert!(audit.session_created(1, "late-session").is_err());
        assert!(ledger
            .task_events()
            .unwrap()
            .iter()
            .all(|event| event.event_type != "task.session"));
    }

    #[test]
    fn prompt_generation_guard_blocks_fail_restart_until_turn_ends() {
        let root = tempfile::tempdir().unwrap();
        let (ledger, audit, _) = setup(root.path());
        audit.session_created(1, "session-one").unwrap();
        audit.session_ready(1, "session-one").unwrap();

        let parent = ledger.last_event_hash().unwrap();
        let failed = new_task_failed_event("main", parent.as_deref(), 1, "restart").unwrap();
        assert!(ledger.append_event(&failed).is_err());
        let parent = ledger.last_event_hash().unwrap();
        let started = new_task_started_event("main", parent.as_deref(), 1, 300, 2).unwrap();
        assert!(ledger.append_event(&started).is_err());
        assert_eq!(ledger.task_views().unwrap().remove(0).attempts, 1);

        drop(audit);
        restart(&ledger);
        assert_eq!(ledger.task_views().unwrap().remove(0).attempts, 2);
    }

    #[test]
    fn controlled_prompt_refuses_fail_restart_after_session_persistence() {
        let root = tempfile::tempdir().unwrap();
        let (ledger, audit, owner) = setup(root.path());
        audit.session_created(1, "session-one").unwrap();
        let persisted = ledger.task_views().unwrap().remove(0);
        assert_eq!(persisted.session_attempt, Some(1));
        assert_eq!(
            persisted.session_lease_owner.as_deref(),
            Some(owner.as_str())
        );
        restart(&ledger);

        assert!(audit.session_ready(1, "session-one").is_err());
    }
}
