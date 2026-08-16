use super::SqliteStore;
use crate::TaskLease;
use rusqlite::{params, OptionalExtension};

impl SqliteStore {
    pub fn task_lease(&self, task_id: u64) -> anyhow::Result<Option<TaskLease>> {
        Ok(self
            .conn
            .query_row(
                "SELECT task_id, attempt, owner, expires_at, heartbeat_at FROM task_leases WHERE task_id = ?1",
                params![task_id],
                |row| {
                    Ok(TaskLease {
                        task_id: row.get(0)?,
                        attempt: row.get(1)?,
                        owner: row.get(2)?,
                        expires_at: row.get(3)?,
                        heartbeat_at: row.get(4)?,
                    })
                },
            )
            .optional()?)
    }

    pub fn upsert_task_lease(&self, lease: &TaskLease) -> anyhow::Result<()> {
        self.conn.execute(
            "INSERT INTO task_leases (task_id, attempt, owner, expires_at, heartbeat_at)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(task_id) DO UPDATE SET
                attempt = excluded.attempt,
                owner = excluded.owner,
                expires_at = excluded.expires_at,
                heartbeat_at = excluded.heartbeat_at",
            params![
                lease.task_id,
                lease.attempt,
                lease.owner,
                lease.expires_at,
                lease.heartbeat_at,
            ],
        )?;
        Ok(())
    }

    pub fn renew_task_lease(
        &self,
        task_id: u64,
        attempt: u32,
        expires_at: &str,
        heartbeat_at: &str,
    ) -> anyhow::Result<bool> {
        Ok(self.conn.execute(
            "UPDATE task_leases SET expires_at = ?3, heartbeat_at = ?4
             WHERE task_id = ?1 AND attempt = ?2",
            params![task_id, attempt, expires_at, heartbeat_at],
        )? == 1)
    }

    pub fn delete_task_lease(&self, task_id: u64, attempt: u32) -> anyhow::Result<bool> {
        Ok(self.conn.execute(
            "DELETE FROM task_leases WHERE task_id = ?1 AND attempt = ?2",
            params![task_id, attempt],
        )? == 1)
    }
}
