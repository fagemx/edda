// ── Data structures ──

#[derive(Debug, Clone)]
pub struct CommitEntry {
    pub ts: String,
    pub event_id: String,
    pub title: String,
    pub purpose: String,
    pub prev_summary: String,
    pub contribution: String,
    pub evidence_lines: Vec<String>,
    pub labels: Vec<String>,
}

#[derive(Debug, Clone)]
pub enum SignalKind {
    NoteTodo,
    NoteDecision,
    CmdFail,
}

#[derive(Debug, Clone)]
pub struct SignalEntry {
    pub ts: String,
    pub kind: SignalKind,
    pub text: String,
    pub event_id: String,
    /// Event ID this decision supersedes (from refs.provenance).
    pub supersedes: Option<String>,
}

#[derive(Debug, Clone)]
pub struct MergeEntry {
    pub ts: String,
    pub event_id: String,
    pub src: String,
    pub dst: String,
    pub reason: String,
    pub adopted_commits: Vec<String>,
}

/// A task snapshot entry within a session digest.
#[derive(Debug, Clone)]
pub struct TaskSnapshotEntry {
    pub subject: String,
    pub status: String,
}

/// A session digest note extracted from the workspace ledger.
pub struct SessionDigestEntry {
    pub ts: String,
    pub event_id: String,
    pub session_id: String,
    pub tool_calls: u64,
    pub tool_failures: u64,
    pub user_prompts: u64,
    pub duration_minutes: u64,
    pub files_modified: Vec<String>,
    pub failed_commands: Vec<String>,
    pub commits_made: Vec<String>,
    pub tasks_snapshot: Vec<TaskSnapshotEntry>,
    /// Session outcome: "completed", "interrupted", or "error_stuck".
    pub outcome: String,
    /// Session notes written by agent via `edda note --tag session`.
    pub notes: Vec<String>,
}

pub struct BranchSnapshot {
    pub branch: String,
    pub created_at: String,
    pub last_event_id: Option<String>,
    pub last_commit_id: Option<String>,
    pub last_commit: Option<CommitEntry>,
    pub commits: Vec<CommitEntry>,
    pub signals: Vec<SignalEntry>,
    pub merges: Vec<MergeEntry>,
    pub session_digests: Vec<SessionDigestEntry>,
    pub uncommitted_events: usize,
}

#[derive(Debug, Clone, Copy)]
pub struct DeriveOptions {
    pub depth: usize,
}

impl Default for DeriveOptions {
    fn default() -> Self {
        Self { depth: 5 }
    }
}
