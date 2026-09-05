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
    /// Times this rule's enforcement was shown to a user (e.g. PreToolUse
    /// matches). Unlike `hits`, shows never update `last_hit`, never promote
    /// Proposed -> Active, and never reactivate Dormant/Settled rules
    /// (GH-813: hit-on-every-Bash-call kept noise rules alive forever).
    #[serde(default)]
    pub shows: u64,
    /// Why this rule was revoked (only set when revoked via `revoke`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revoked_reason: Option<String>,
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

    /// Record that this rule's enforcement was shown to the user.
    ///
    /// Increments the show counter only: `last_hit` is NOT updated, Proposed
    /// rules are NOT promoted, and Dormant/Settled rules are NOT reactivated
    /// (GH-813: shows are pure statistics and must not reset the TTL).
    pub fn record_shown(&mut self) {
        self.shows += 1;
    }

    /// Revoke this rule: mark it Dead with a reason.
    pub fn revoke(&mut self, reason: String) {
        self.status = RuleStatus::Dead;
        self.revoked_reason = Some(reason);
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
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RulesStore {
    pub rules: Vec<Rule>,
    #[serde(default)]
    pub last_decay_run: Option<String>,
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
            shows: 0,
            revoked_reason: None,
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
        // 0. Reclaim disallowed command triggers (GH-813): shell
        // builtins/keywords/common utilities and variable assignments never
        // make meaningful learned rules — revoke them outright.
        for rule in &mut self.rules {
            if rule.is_alive() && is_disallowed_trigger(&rule.trigger) {
                rule.revoke("disallowed command trigger (builtin/keyword/assignment)".to_string());
            }
        }

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
                    if current_hash != *stored_hash && rule.status == RuleStatus::Active {
                        rule.status = RuleStatus::Dormant;
                    }
                } else if !Path::new(anchor_file).exists() && rule.status == RuleStatus::Active {
                    rule.status = RuleStatus::Dormant;
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
        active_ids.sort_by_key(|entry| std::cmp::Reverse(entry.1)); // Sort by hits descending
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

    /// Record shows for matched rules. Unlike `record_matched_hits`, this
    /// never resets the TTL or reactivates decayed rules (GH-813).
    pub fn record_matched_shows(&mut self, matched_ids: &[String]) {
        for id in matched_ids {
            if let Some(rule) = self.get_mut(id) {
                rule.record_shown();
            }
        }
    }

    /// Revoke a rule by ID. Returns false when the ID is unknown.
    pub fn revoke_rule(&mut self, id: &str, reason: String) -> bool {
        match self.get_mut(id) {
            Some(rule) => {
                rule.revoke(reason);
                true
            }
            None => false,
        }
    }

    /// Garbage-collect dead rules (remove from store entirely).
    pub fn gc_dead_rules(&mut self) -> usize {
        let before = self.rules.len();
        self.rules.retain(|r| !matches!(r.status, RuleStatus::Dead));
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

/// Shell builtins, keywords, and ubiquitous utilities whose failures are
/// environmental noise rather than a missing-tool signal. They must never
/// become learned-rule command triggers (GH-813: echo 1789 / cd 1618 hits).
pub const DISALLOWED_TRIGGER_WORDS: &[&str] = &[
    "alias",
    "bg",
    "bind",
    "break",
    "builtin",
    "caller",
    "case",
    "cd",
    "command",
    "compgen",
    "complete",
    "compopt",
    "continue",
    "coproc",
    "declare",
    "dirs",
    "disown",
    "do",
    "done",
    "echo",
    "elif",
    "else",
    "enable",
    "esac",
    "eval",
    "exec",
    "exit",
    "export",
    "fc",
    "fg",
    "fi",
    "for",
    "function",
    "getopts",
    "hash",
    "help",
    "history",
    "if",
    "in",
    "jobs",
    "kill",
    "let",
    "local",
    "logout",
    "mapfile",
    "popd",
    "printf",
    "pushd",
    "pwd",
    "read",
    "readarray",
    "readonly",
    "return",
    "select",
    "set",
    "shift",
    "shopt",
    "source",
    "suspend",
    "test",
    "then",
    "time",
    "times",
    "trap",
    "true",
    "type",
    "typeset",
    "ulimit",
    "umask",
    "unalias",
    "unset",
    "until",
    "wait",
    "while",
    "cat",
    "sed",
    "grep",
    "head",
    "tail",
    "wc",
    "find",
    "ls",
    "false",
];

/// True when a bare token is a variable assignment: `NAME=value` or the
/// bash append form `NAME+=value`. The name before `=` / `+=` must be a
/// valid shell identifier (ASCII alpha/underscore, then ASCII alphanum or
/// underscore).
pub fn is_var_assignment(token: &str) -> bool {
    let Some(eq) = token.find('=') else {
        return false;
    };
    let name = &token[..eq];
    let name = name.strip_suffix('+').unwrap_or(name);
    !name.is_empty()
        && name
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Split a shell command into segments on `;`, `&`, `|`, and newline.
///
/// Quote-aware: delimiters inside single quotes, double quotes, or after a
/// backslash escape do not split. For example
/// `printf '%s' 'skip; python -V'` is ONE segment whose command word is
/// `printf`.
pub fn split_command_segments(cmd: &str) -> Vec<&str> {
    let mut segments = Vec::new();
    let mut start = 0usize;
    let mut in_single = false;
    let mut in_double = false;
    let mut escaped = false;
    for (i, ch) in cmd.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        match ch {
            '\\' if !in_single => escaped = true,
            '\'' if !in_double => in_single = !in_single,
            '"' if !in_single => in_double = !in_double,
            ';' | '&' | '|' | '\n' if !in_single && !in_double => {
                segments.push(&cmd[start..i]);
                start = i + ch.len_utf8();
            }
            _ => {}
        }
    }
    segments.push(&cmd[start..]);
    segments
}

/// Tokenize a command segment into unquoted words.
///
/// Splits on unquoted whitespace; single and double quotes group characters
/// into one word and are stripped (so `'python'` yields `python`); a
/// backslash outside single quotes escapes the next character (so
/// `git\ commit` is one word). Inside double quotes only `\`, `"`, `$`, and
/// backtick drop the backslash, matching shell quoting rules closely enough
/// for command-word extraction.
fn unquoted_words(segment: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut current = String::new();
    let mut in_word = false;
    let mut in_single = false;
    let mut in_double = false;
    let mut escaped = false;
    for ch in segment.chars() {
        if escaped {
            // Outside quotes any escaped character is literal; inside double
            // quotes only a few escapes drop the backslash.
            if !in_double || matches!(ch, '\\' | '"' | '$' | '`') {
                current.push(ch);
            } else {
                current.push('\\');
                current.push(ch);
            }
            escaped = false;
            continue;
        }
        match ch {
            '\\' if !in_single => {
                escaped = true;
                in_word = true;
            }
            '\'' if !in_double => {
                in_single = !in_single;
                in_word = true;
            }
            '"' if !in_single => {
                in_double = !in_double;
                in_word = true;
            }
            c if c.is_whitespace() && !in_single && !in_double => {
                if in_word {
                    words.push(std::mem::take(&mut current));
                    in_word = false;
                }
            }
            c => {
                current.push(c);
                in_word = true;
            }
        }
    }
    if in_word {
        words.push(current);
    }
    words
}

/// Quote-aware command word of a command segment.
///
/// Tokenizes `segment` into words while respecting quotes and backslash
/// escapes, skips leading variable assignments (including quoted values
/// like `FOO='hello world'`), and returns the first non-assignment word,
/// unquoted (e.g. `python` for `'python'` or `"python"`). Returns None when
/// the segment is empty or consists only of assignments.
pub fn command_word(segment: &str) -> Option<String> {
    let mut words = unquoted_words(segment);
    while words.first().is_some_and(|w| is_var_assignment(w)) {
        words.remove(0);
    }
    words.into_iter().next()
}

/// True when a command may become a learned-rule command trigger.
///
/// Rejects empty/whitespace commands, compound commands (`;`, `&&`, `||`,
/// `|`, newline), commands whose leading word is a variable assignment
/// (`FOO=bar npm test` keeps its assignment prefix out of learned
/// triggers), and shell builtins/keywords/common utilities (GH-813). The
/// leading word is taken quote-aware, so `"echo" hi` is also rejected.
pub fn is_trackable_command(cmd: &str) -> bool {
    let cmd = cmd.trim();
    if cmd.is_empty() || cmd.contains([';', '|', '&', '\n']) {
        return false;
    }
    match unquoted_words(cmd).first() {
        Some(word) if !is_var_assignment(word) => {
            !DISALLOWED_TRIGGER_WORDS.contains(&word.as_str())
        }
        _ => false,
    }
}

/// True when a stored rule trigger is disallowed and should be reclaimed by
/// the decay cycle. ONLY `command_failure:` triggers can be disallowed
/// command triggers: `file_churn:<path>` (paths may contain `=`),
/// `multi_agent_start`, and free-text triggers are never revoked by the
/// decay cycle (GH-813).
pub fn is_disallowed_trigger(trigger: &str) -> bool {
    let Some(cmd) = trigger.strip_prefix("command_failure:") else {
        return false;
    };
    cmd.contains('=') || !is_trackable_command(cmd)
}

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

#[path = "rules_tests.rs"]
#[cfg(test)]
mod tests;
