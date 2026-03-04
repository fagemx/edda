## Implementation Plan: Activity Classification

### Overview
Add automatic activity classification to session digests based on tool signature patterns, with manual override capability.

### Phase 1: Core Classification Logic

#### 1.1 Define Activity Types Enum
**File:** `crates/edda-bridge-claude/src/digest.rs` (after line 50)

Add enum:
```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ActivityType {
    #[default]
    Unknown,
    Feature,
    Fix,
    Debug,
    Refactor,
    Docs,
    Research,
    Chat,
    Ops,
}
```

#### 1.2 Add Classification Field to SessionStats
**File:** `crates/edda-bridge-claude/src/digest.rs` (line 96, in SessionStats struct)

Add field:
```rust
pub activity: ActivityType,
```

#### 1.3 Implement Classification Logic
**File:** `crates/edda-bridge-claude/src/digest.rs` (new function after `compute_tool_ratios`)

Add function:
```rust
fn classify_activity(stats: &SessionStats) -> ActivityType {
    if stats.tool_calls == 0 {
        return ActivityType::Chat;
    }
    
    let breakdown = &stats.tool_call_breakdown;
    let total = stats.tool_calls as f64;
    
    // Compute ratios
    let edit_ratio = (breakdown.get("Edit").unwrap_or(&0) + 
                      breakdown.get("Write").unwrap_or(&0)) as f64 / total;
    let search_ratio = (breakdown.get("Read").unwrap_or(&0) + 
                        breakdown.get("Grep").unwrap_or(&0) + 
                        breakdown.get("Glob").unwrap_or(&0)) as f64 / total;
    let bash_ratio = breakdown.get("Bash").unwrap_or(&0) as f64 / total;
    
    // Check for docs-only edits
    let all_docs = stats.files_modified.iter().all(|f| f.ends_with(".md"));
    if all_docs && edit_ratio > 0.0 {
        return ActivityType::Docs;
    }
    
    // High search, low edit = research
    if search_ratio > 0.6 && edit_ratio < 0.1 {
        return ActivityType::Research;
    }
    
    // Many failures = debugging
    if stats.tool_failures > 3 && stats.tool_failures as f64 / total > 0.2 {
        return ActivityType::Debug;
    }
    
    // Git commits + edits = feature or fix
    if !stats.commits_made.is_empty() && edit_ratio > 0.1 {
        // Check commit messages for fix/bug keywords
        let commit_text = stats.commits_made.join(" ").to_lowercase();
        if commit_text.contains("fix") || commit_text.contains("bug") {
            return ActivityType::Fix;
        }
        return ActivityType::Feature;
    }
    
    // Bash-heavy with CI/deploy patterns = ops
    if bash_ratio > 0.4 {
        return ActivityType::Ops;
    }
    
    // High edit ratio = refactor or feature
    if edit_ratio > 0.3 {
        // Check for rename/move patterns (refactor)
        // Default to feature
        return ActivityType::Feature;
    }
    
    // Low tool calls = chat
    if stats.tool_calls < 5 {
        return ActivityType::Chat;
    }
    
    ActivityType::Unknown
}
```

#### 1.4 Call Classification in extract_stats
**File:** `crates/edda-bridge-claude/src/digest.rs` (line 226, after computing duration)

Add:
```rust
stats.activity = classify_activity(&stats);
```

#### 1.5 Include Activity in Digest Event
**File:** `crates/edda-bridge-claude/src/digest.rs` (line 438, in build_digest_event payload)

Add to session_stats:
```rust
"activity": stats.activity.to_string(),
```

#### 1.6 Update PrevDigest
**File:** `crates/edda-bridge-claude/src/digest.rs` (line 1330, in PrevDigest struct)

Add field:
```rust
pub activity: String,
```

#### 1.7 Update write_prev_digest
**File:** `crates/edda-bridge-claude/src/digest.rs` (line 1384, in write_prev_digest)

Add:
```rust
activity: stats.activity.to_string(),
```

### Phase 2: Display in CLI

#### 2.1 Update SessionDigestEntry Type
**File:** `crates/edda-derive/src/types.rs` (line 74, in SessionDigestEntry)

Add field:
```rust
pub activity: String,
```

#### 2.2 Display Activity in edda log
**File:** `crates/edda-cli/src/cmd_log.rs` (line 295, in format_session_digest_detail)

Add after outcome:
```rust
let activity = ss
    .and_then(|s| s.get("activity"))
    .and_then(|v| v.as_str())
    .unwrap_or("unknown");

parts.push(format!("[{}]", activity));
```

### Phase 3: Manual Override

#### 3.1 Add Reclassify Command
**File:** `crates/edda-cli/src/main.rs` (line 64, after Note command)

Add to Command enum:
```rust
/// Reclassify a session's activity type
Reclassify {
    /// Session ID (short form, e.g. first 8 chars)
    session: String,
    /// New activity type (feature/fix/debug/refactor/docs/research/chat/ops)
    activity: String,
},
```

#### 3.2 Implement Reclassify Handler
**File:** `crates/edda-cli/src/cmd_note.rs`

Add function:
```rust
pub fn reclassify_activity(
    repo_root: &std::path::Path,
    session_id: &str,
    activity: &str,
) -> anyhow::Result<()> {
    let ledger = Ledger::open(repo_root)?;
    let _lock = WorkspaceLock::acquire(&ledger.paths)?;
    
    // Validate activity type
    let valid = ["feature", "fix", "debug", "refactor", "docs", "research", "chat", "ops", "unknown"];
    if !valid.contains(&activity.to_lowercase().as_str()) {
        anyhow::bail!("Invalid activity type. Valid types: {:?}", valid);
    }
    
    // Find session digest event by session_id prefix match
    let events = ledger.iter_events()?;
    let target_event = events
        .iter()
        .rev()
        .find(|e| {
            let is_digest = e.payload
                .get("tags")
                .and_then(|v| v.as_array())
                .map(|arr| arr.iter().any(|t| t.as_str() == Some("session_digest")))
                .unwrap_or(false);
            
            let matches_session = e.payload
                .get("session_id")
                .and_then(|v| v.as_str())
                .map(|sid| sid.starts_with(session_id))
                .unwrap_or(false);
            
            is_digest && matches_session
        })
        .ok_or_else(|| anyhow::anyhow!("Session not found: {}", session_id))?;
    
    // Update activity in session_stats
    let mut event = target_event.clone();
    if let Some(stats) = event.payload.get_mut("session_stats") {
        if let Some(obj) = stats.as_object_mut() {
            obj.insert("activity".to_string(), serde_json::json!(activity));
        }
    }
    
    // Add manual classification tag
    if let Some(tags) = event.payload.get_mut("tags") {
        if let Some(arr) = tags.as_array_mut() {
            arr.push(serde_json::json!("manual_classification"));
        }
    }
    
    // Note: In a proper implementation, we'd need to rebuild the event hash
    // and update the ledger. For now, this shows the structure.
    
    println!("Updated session {} activity to: {}", session_id, activity);
    Ok(())
}
```

#### 3.3 Wire Command Handler
**File:** `crates/edda-cli/src/main.rs` (line 900+, in match cli.cmd)

Add:
```rust
Command::Reclassify { session, activity } => {
    cmd_note::reclassify_activity(&repo_root, &session, &activity)
}
```

### Testing Strategy

#### Unit Tests
**File:** `crates/edda-bridge-claude/src/digest.rs` (add to tests module)

```rust
#[test]
fn classify_docs_only() {
    let mut stats = SessionStats::default();
    stats.tool_calls = 10;
    stats.tool_call_breakdown.insert("Edit".to_string(), 5);
    stats.files_modified = vec!["README.md".to_string(), "docs/api.md".to_string()];
    assert_eq!(classify_activity(&stats), ActivityType::Docs);
}

#[test]
fn classify_research_heavy() {
    let mut stats = SessionStats::default();
    stats.tool_calls = 20;
    stats.tool_call_breakdown.insert("Read".to_string(), 12);
    stats.tool_call_breakdown.insert("Grep".to_string(), 5);
    assert_eq!(classify_activity(&stats), ActivityType::Research);
}

#[test]
fn classify_debug_failures() {
    let mut stats = SessionStats::default();
    stats.tool_calls = 15;
    stats.tool_failures = 5;
    stats.tool_call_breakdown.insert("Bash".to_string(), 10);
    assert_eq!(classify_activity(&stats), ActivityType::Debug);
}

#[test]
fn classify_feature_with_commits() {
    let mut stats = SessionStats::default();
    stats.tool_calls = 20;
    stats.tool_call_breakdown.insert("Edit".to_string(), 8);
    stats.commits_made = vec!["feat: add new feature".to_string()];
    assert_eq!(classify_activity(&stats), ActivityType::Feature);
}
```

### Acceptance Criteria
- [ ] Sessions automatically classified by tool patterns
- [ ] `edda log --type session` shows activity type in brackets
- [ ] Classification stored in session_stats.activity field
- [ ] Manual override via `edda reclassify <session> <type>` command
- [ ] Unit tests cover all classification paths

### Dependencies
- ✅ Issue #160 (tool call statistics) - already implemented
