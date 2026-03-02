//! Rules store with immune-system lifecycle and TTL decay.
//!
//! Rules are learned from post-mortem analysis and enforced via hooks.
//! Each rule follows an immune-system lifecycle:
//!
//!   Proposed -> Active -> Dormant -> Settled -> Dead
//!                                           |
//!                              Superseded --+
//!
//! Three decay mechanisms:
//! - **Time decay**: TTL (default 30 days), reset on each trigger hit
//! - **Anchor decay**: Rule anchored to file; file changes -> stale
//! - **Contradiction detection**: Same trigger, different action -> supersede

use serde::{Deserialize, Serialize};
use sha2::{Digest as Sha2Digest, Sha256};
use std::fs;
use std::path::{Path, PathBuf};
use time::OffsetDateTime;

/// Default TTL in days for new rules.
const DEFAULT_TTL_DAYS: u32 = 30;

/// Maximum number of active rules enforced simultaneously.
const MAX_ACTIVE_RULES: usize = 15;

/// Days after last_hit before a rule transitions from Active -> Dormant.
const DORMANT_THRESHOLD_DAYS: i64 = 30;

/// Days after last_hit before Dormant -> Settled.
const SETTLED_THRESHOLD_DAYS: i64 = 60;

/// Days after last_hit before Settled -> Dead.
const DEAD_THRESHOLD_DAYS: i64 = 90;

/// Minimum confirmations to promote Proposed -> Active.
const MIN_CONFIRMATIONS: u64 = 2;

/// Rule lifecycle status (immune system model).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RuleStatus {
    /// First observation, needs confirmation (pattern repeated 2x to activate).
    Proposed,
    /// Pattern confirmed, rule is enforced.
    Active,
    /// TTL window passed without trigger; rule is suspended.
    Dormant,
    /// Long dormant, near death.
    Settled,
    /// TTL expired completely; rule is archived.
    Dead,
    /// Contradicted by a newer rule with the same trigger.
    Superseded,
}

impl std::fmt::Display for RuleStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Proposed => write!(f, "proposed"),
            Self::Active => write!(f, "active"),
            Self::Dormant => write!(f, "dormant"),
            Self::Settled => write!(f, "settled"),
            Self::Dead => write!(f, "dead"),
            Self::Superseded => write!(f, "superseded"),
        }
    }
}

/// What kind of rule this is — determines enforcement mechanism.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RuleCategory {
    /// Check before commit (PreCommit hook).
    PreCommit,
    /// Check before push (PrePush hook).
    PrePush,
    /// Code pattern to avoid/enforce.
    CodePattern,
    /// Workflow pattern to follow.
    Workflow,
}

impl std::fmt::Display for RuleCategory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::PreCommit => write!(f, "pre_commit"),
            Self::PrePush => write!(f, "pre_push"),
            Self::CodePattern => write!(f, "code_pattern"),
            Self::Workflow => write!(f, "workflow"),
        }
    }
}

/// A learned rule with TTL decay.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Rule {
    pub id: String,
    pub trigger: String,
    pub action: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub anchor_file: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub anchor_hash: Option<String>,
    pub created: String,
    pub last_hit: String,
    pub hits: u64,
    pub ttl_days: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub superseded_by: Option<String>,
    pub status: RuleStatus,
    pub source_session: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_event: Option<String>,
    pub category: RuleCategory,
}

impl Rule {
    /// Check if this rule is enforceable (Active status).
    pub fn is_enforceable(&self) -> bool {
        self.status == RuleStatus::Active
    }

    /// Check if this rule is alive (not Dead or Superseded).
    pub fn is_alive(&self) -> bool {
        !matches!(self.status, RuleStatus::Dead | RuleStatus::Superseded)
    }

    /// Record a trigger hit: increment counter and reset TTL.
    pub fn record_hit(&mut self) {
        self.hits += 1;
        self.last_hit = now_rfc3339();
        // If proposed and enough hits, promote to active
        if self.status == RuleStatus::Proposed && self.hits >= MIN_CONFIRMATIONS {
            self.status = RuleStatus::Active;
        }
        // If dormant/settled, reactivate on hit
        if matches!(self.status, RuleStatus::Dormant | RuleStatus::Settled) {
            self.status = RuleStatus::Active;
        }
    }

    /// Compute days since last hit.
    pub fn days_since_last_hit(&self) -> Option<i64> {
        let last = parse_rfc3339(&self.last_hit)?;
        let now = OffsetDateTime::now_utc();
        Some((now - last).whole_days())
    }

    /// Apply time-based decay to this rule's status.
    pub fn apply_time_decay(&mut self) {
        if matches!(
            self.status,
            RuleStatus::Dead | RuleStatus::Superseded | RuleStatus::Proposed
        ) {
            return;
        }

        let days = match self.days_since_last_hit() {
            Some(d) => d,
            None => return,
        };

        if days >= DEAD_THRESHOLD_DAYS {
            self.status = RuleStatus::Dead;
        } else if days >= SETTLED_THRESHOLD_DAYS {
            self.status = RuleStatus::Settled;
        } else if days >= DORMANT_THRESHOLD_DAYS {
            self.status = RuleStatus::Dormant;
        }
        // else: still Active, no change
    }
}

/// The rules store: manages rules.json persistence and lifecycle.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RulesStore {
    pub rules: Vec<Rule>,
    #[serde(default)]
    pub last_decay_run: Option<String>,
}

impl Default for RulesStore {
    fn default() -> Self {
        Self {
            rules: Vec::new(),
            last_decay_run: None,
        }
    }
}

impl RulesStore {
    /// Load rules store from disk. Returns default if file doesn't exist.
    pub fn load(path: &Path) -> Self {
        match fs::read_to_string(path) {
            Ok(content) => serde_json::from_str(&content).unwrap_or_default(),
            Err(_) => Self::default(),
        }
    }

    /// Persist rules store to disk atomically.
    pub fn save(&self, path: &Path) -> anyhow::Result<()> {
        let json = serde_json::to_string_pretty(self)?;
        edda_store::write_atomic(path, json.as_bytes())
    }

    /// Resolve the rules.json path for a project.
    pub fn project_rules_path(project_id: &str) -> PathBuf {
        edda_store::project_dir(project_id)
            .join("state")
            .join("rules.json")
    }

    /// Resolve the global rules.json path (~/.edda/rules.json).
    pub fn global_rules_path() -> PathBuf {
        edda_store::store_root().join("rules.json")
    }

    /// Load project-scoped rules.
    pub fn load_project(project_id: &str) -> Self {
        Self::load(&Self::project_rules_path(project_id))
    }

    /// Save project-scoped rules.
    pub fn save_project(&self, project_id: &str) -> anyhow::Result<()> {
        self.save(&Self::project_rules_path(project_id))
    }

    /// Get all active (enforceable) rules.
    pub fn active_rules(&self) -> Vec<&Rule> {
        self.rules.iter().filter(|r| r.is_enforceable()).collect()
    }

    /// Get all alive rules (not dead/superseded).
    pub fn alive_rules(&self) -> Vec<&Rule> {
        self.rules.iter().filter(|r| r.is_alive()).collect()
    }

    /// Add a new rule proposal. If a rule with the same trigger already exists
    /// and is alive, increment its hits instead (confirmation).
    pub fn propose_rule(
        &mut self,
        trigger: String,
        action: String,
        anchor_file: Option<String>,
        category: RuleCategory,
        source_session: String,
        source_event: Option<String>,
    ) -> String {
        // Check for contradiction: same trigger, different action -> supersede old
        let mut superseded_ids = Vec::new();
        for rule in &self.rules {
            if rule.trigger == trigger && rule.is_alive() {
                if rule.action == action {
                    // Same trigger + same action: confirmation, not new rule.
                    // Find the mutable reference and record hit.
                    let rule_id = rule.id.clone();
                    if let Some(existing) = self.rules.iter_mut().find(|r| r.id == rule_id) {
                        existing.record_hit();
                    }
                    return rule_id;
                }
                // Same trigger, different action -> contradiction
                superseded_ids.push(rule.id.clone());
            }
        }

        // Supersede contradicting rules
        let new_id = new_rule_id();
        for sid in &superseded_ids {
            if let Some(old_rule) = self.rules.iter_mut().find(|r| r.id == *sid) {
                old_rule.status = RuleStatus::Superseded;
                old_rule.superseded_by = Some(new_id.clone());
            }
        }

        // Compute anchor hash if anchor file provided
        let anchor_hash = anchor_file.as_ref().and_then(|f| file_sha256(f));

        let now = now_rfc3339();
        let rule = Rule {
            id: new_id.clone(),
            trigger,
            action,
            anchor_file,
            anchor_hash,
            created: now.clone(),
            last_hit: now,
            hits: 1,
            ttl_days: DEFAULT_TTL_DAYS,
            superseded_by: None,
            status: RuleStatus::Proposed,
            source_session,
            source_event,
            category,
        };

        self.rules.push(rule);
        new_id
    }

    /// Run the full decay cycle on all rules.
    ///
    /// 1. Time decay: check TTL against last_hit
    /// 2. Anchor decay: check if anchored file changed
    /// 3. Enforce active window cap (~15)
    pub fn run_decay_cycle(&mut self) {
        // 1. Time decay
        for rule in &mut self.rules {
            rule.apply_time_decay();
        }

        // 2. Anchor decay: mark rules stale if anchored file changed
        for rule in &mut self.rules {
            if !rule.is_alive() {
                continue;
            }
            if let (Some(ref anchor_file), Some(ref stored_hash)) =
                (&rule.anchor_file, &rule.anchor_hash)
            {
                if let Some(current_hash) = file_sha256(anchor_file) {
                    if current_hash != *stored_hash {
                        // File changed significantly -> mark dormant
                        if rule.status == RuleStatus::Active {
                            rule.status = RuleStatus::Dormant;
                        }
                    }
                }
                // If file was deleted, also mark dormant
                else if !Path::new(anchor_file).exists() {
                    if rule.status == RuleStatus::Active {
                        rule.status = RuleStatus::Dormant;
                    }
                }
            }
        }

        // 3. Enforce active window cap: keep top N by hits, demote rest
        let mut active_ids: Vec<(String, u64)> = self
            .rules
            .iter()
            .filter(|r| r.status == RuleStatus::Active)
            .map(|r| (r.id.clone(), r.hits))
            .collect();
        active_ids.sort_by(|a, b| b.1.cmp(&a.1)); // Sort by hits descending
        if active_ids.len() > MAX_ACTIVE_RULES {
            let demote_ids: Vec<String> = active_ids[MAX_ACTIVE_RULES..]
                .iter()
                .map(|(id, _)| id.clone())
                .collect();
            for rule in &mut self.rules {
                if demote_ids.contains(&rule.id) {
                    rule.status = RuleStatus::Dormant;
                }
            }
        }

        self.last_decay_run = Some(now_rfc3339());
    }

    /// Garbage-collect dead rules (remove from store entirely).
    pub fn gc_dead_rules(&mut self) -> usize {
        let before = self.rules.len();
        self.rules
            .retain(|r| !matches!(r.status, RuleStatus::Dead));
        before - self.rules.len()
    }

    /// Find rules matching a given trigger pattern (substring match).
    pub fn find_by_trigger(&self, trigger_pattern: &str) -> Vec<&Rule> {
        self.rules
            .iter()
            .filter(|r| r.trigger.contains(trigger_pattern))
            .collect()
    }

    /// Get a rule by ID.
    pub fn get(&self, id: &str) -> Option<&Rule> {
        self.rules.iter().find(|r| r.id == id)
    }

    /// Get a mutable rule by ID.
    pub fn get_mut(&mut self, id: &str) -> Option<&mut Rule> {
        self.rules.iter_mut().find(|r| r.id == id)
    }

    /// Summary statistics.
    pub fn stats(&self) -> StoreStats {
        let mut stats = StoreStats::default();
        for rule in &self.rules {
            match rule.status {
                RuleStatus::Proposed => stats.proposed += 1,
                RuleStatus::Active => stats.active += 1,
                RuleStatus::Dormant => stats.dormant += 1,
                RuleStatus::Settled => stats.settled += 1,
                RuleStatus::Dead => stats.dead += 1,
                RuleStatus::Superseded => stats.superseded += 1,
            }
        }
        stats.total = self.rules.len();
        stats
    }
}

/// Summary statistics for the rules store.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct StoreStats {
    pub total: usize,
    pub proposed: usize,
    pub active: usize,
    pub dormant: usize,
    pub settled: usize,
    pub dead: usize,
    pub superseded: usize,
}

// -- Helpers --

fn new_rule_id() -> String {
    format!("rule_{}", ulid::Ulid::new().to_string().to_lowercase())
}

fn now_rfc3339() -> String {
    let now = OffsetDateTime::now_utc();
    now.format(&time::format_description::well_known::Rfc3339)
        .expect("RFC3339 formatting should not fail")
}

fn parse_rfc3339(s: &str) -> Option<OffsetDateTime> {
    OffsetDateTime::parse(s, &time::format_description::well_known::Rfc3339).ok()
}

/// Compute SHA-256 of a file's contents. Returns None if file unreadable.
fn file_sha256(path: &str) -> Option<String> {
    let data = fs::read(path).ok()?;
    let hash = Sha256::digest(&data);
    Some(hex::encode(hash))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_rule(trigger: &str, action: &str, status: RuleStatus) -> Rule {
        Rule {
            id: new_rule_id(),
            trigger: trigger.to_string(),
            action: action.to_string(),
            anchor_file: None,
            anchor_hash: None,
            created: now_rfc3339(),
            last_hit: now_rfc3339(),
            hits: 2,
            ttl_days: DEFAULT_TTL_DAYS,
            superseded_by: None,
            status,
            source_session: "test-session".to_string(),
            source_event: None,
            category: RuleCategory::PreCommit,
        }
    }

    #[test]
    fn new_store_is_empty() {
        let store = RulesStore::default();
        assert!(store.rules.is_empty());
        assert!(store.active_rules().is_empty());
    }

    #[test]
    fn propose_creates_proposed_rule() {
        let mut store = RulesStore::default();
        let id = store.propose_rule(
            "test failure".into(),
            "run tests before commit".into(),
            None,
            RuleCategory::PreCommit,
            "s1".into(),
            None,
        );
        assert!(!id.is_empty());
        let rule = store.get(&id).unwrap();
        assert_eq!(rule.status, RuleStatus::Proposed);
        assert_eq!(rule.hits, 1);
    }

    #[test]
    fn duplicate_proposal_confirms_existing() {
        let mut store = RulesStore::default();
        let id1 = store.propose_rule(
            "test failure".into(),
            "run tests before commit".into(),
            None,
            RuleCategory::PreCommit,
            "s1".into(),
            None,
        );
        let id2 = store.propose_rule(
            "test failure".into(),
            "run tests before commit".into(),
            None,
            RuleCategory::PreCommit,
            "s2".into(),
            None,
        );
        // Same rule ID returned (confirmation, not new rule)
        assert_eq!(id1, id2);
        let rule = store.get(&id1).unwrap();
        assert_eq!(rule.hits, 2);
        // 2 hits -> promoted to Active
        assert_eq!(rule.status, RuleStatus::Active);
    }

    #[test]
    fn contradiction_supersedes_old_rule() {
        let mut store = RulesStore::default();
        let id1 = store.propose_rule(
            "test failure".into(),
            "run tests before commit".into(),
            None,
            RuleCategory::PreCommit,
            "s1".into(),
            None,
        );
        // Same trigger, different action
        let id2 = store.propose_rule(
            "test failure".into(),
            "run linter before commit".into(),
            None,
            RuleCategory::PreCommit,
            "s2".into(),
            None,
        );
        assert_ne!(id1, id2);
        let old = store.get(&id1).unwrap();
        assert_eq!(old.status, RuleStatus::Superseded);
        assert_eq!(old.superseded_by.as_deref(), Some(id2.as_str()));
    }

    #[test]
    fn record_hit_resets_ttl() {
        let mut rule = make_rule("trigger", "action", RuleStatus::Active);
        let before = rule.last_hit.clone();
        std::thread::sleep(std::time::Duration::from_millis(10));
        rule.record_hit();
        assert_ne!(rule.last_hit, before);
        assert_eq!(rule.hits, 3);
    }

    #[test]
    fn dormant_reactivates_on_hit() {
        let mut rule = make_rule("trigger", "action", RuleStatus::Dormant);
        rule.record_hit();
        assert_eq!(rule.status, RuleStatus::Active);
    }

    #[test]
    fn active_window_cap_enforced() {
        let mut store = RulesStore::default();
        // Create 20 active rules
        for i in 0..20 {
            let mut rule = make_rule(
                &format!("trigger_{i}"),
                &format!("action_{i}"),
                RuleStatus::Active,
            );
            rule.hits = 20 - i;
            store.rules.push(rule);
        }
        store.run_decay_cycle();
        let active_count = store.active_rules().len();
        assert!(
            active_count <= MAX_ACTIVE_RULES,
            "active_count={active_count} exceeds cap={MAX_ACTIVE_RULES}"
        );
    }

    #[test]
    fn gc_removes_dead_rules() {
        let mut store = RulesStore::default();
        store
            .rules
            .push(make_rule("a", "b", RuleStatus::Active));
        store.rules.push(make_rule("c", "d", RuleStatus::Dead));
        store
            .rules
            .push(make_rule("e", "f", RuleStatus::Superseded));
        let removed = store.gc_dead_rules();
        assert_eq!(removed, 1);
        assert_eq!(store.rules.len(), 2);
    }

    #[test]
    fn store_round_trip() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("rules.json");

        let mut store = RulesStore::default();
        store.propose_rule(
            "trigger".into(),
            "action".into(),
            None,
            RuleCategory::Workflow,
            "s1".into(),
            None,
        );
        store.save(&path).unwrap();

        let loaded = RulesStore::load(&path);
        assert_eq!(loaded.rules.len(), 1);
        assert_eq!(loaded.rules[0].trigger, "trigger");
    }

    #[test]
    fn stats_counts_correctly() {
        let mut store = RulesStore::default();
        store
            .rules
            .push(make_rule("a", "b", RuleStatus::Active));
        store
            .rules
            .push(make_rule("c", "d", RuleStatus::Proposed));
        store
            .rules
            .push(make_rule("e", "f", RuleStatus::Dormant));
        store.rules.push(make_rule("g", "h", RuleStatus::Dead));
        let stats = store.stats();
        assert_eq!(stats.total, 4);
        assert_eq!(stats.active, 1);
        assert_eq!(stats.proposed, 1);
        assert_eq!(stats.dormant, 1);
        assert_eq!(stats.dead, 1);
    }
}
