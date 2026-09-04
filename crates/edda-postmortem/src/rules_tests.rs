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
        shows: 0,
        revoked_reason: None,
        category: RuleCategory::PreCommit,
    }
}

fn rfc3339_days_ago(days: i64) -> String {
    (OffsetDateTime::now_utc() - time::Duration::days(days))
        .format(&time::format_description::well_known::Rfc3339)
        .expect("RFC3339 formatting should not fail")
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
    store.rules.push(make_rule("a", "b", RuleStatus::Active));
    store.rules.push(make_rule("c", "d", RuleStatus::Dead));
    store
        .rules
        .push(make_rule("e", "f", RuleStatus::Superseded));
    let removed = store.gc_dead_rules();
    assert_eq!(removed, 1);
    assert_eq!(store.rules.len(), 2);
}

#[test]
fn record_shown_never_resets_ttl_or_promotes() {
    // Proposed rule shown 100 times: still Proposed, hits untouched.
    let mut proposed = make_rule("trigger", "action", RuleStatus::Proposed);
    for _ in 0..100 {
        proposed.record_shown();
    }
    assert_eq!(proposed.shows, 100);
    assert_eq!(proposed.hits, 2);
    assert_eq!(proposed.status, RuleStatus::Proposed);

    // Active rule whose TTL already elapsed still decays on schedule:
    // shows never refresh last_hit (GH-813).
    let mut active = make_rule("trigger", "action", RuleStatus::Active);
    active.last_hit = rfc3339_days_ago(31);
    let last_hit = active.last_hit.clone();
    for _ in 0..100 {
        active.record_shown();
    }
    active.apply_time_decay();
    assert_eq!(active.shows, 100);
    assert_eq!(active.status, RuleStatus::Dormant);
    assert_eq!(active.last_hit, last_hit);
}

#[test]
fn revoke_marks_dead_with_reason() {
    let mut store = RulesStore::default();
    store.rules.push(make_rule("a", "b", RuleStatus::Active));
    let id = store.rules[0].id.clone();
    assert!(store.revoke_rule(&id, "superseded by policy".to_string()));
    let rule = store.get(&id).unwrap();
    assert_eq!(rule.status, RuleStatus::Dead);
    assert_eq!(rule.revoked_reason.as_deref(), Some("superseded by policy"));
    assert!(!store.revoke_rule("rule_missing", "x".to_string()));
}

#[test]
fn decay_cycle_reclaims_disallowed_command_triggers() {
    let mut store = RulesStore::default();
    for trigger in [
        "command_failure:cd",
        "command_failure:echo",
        "command_failure:ls",
        "command_failure:FOO=1",
        "command_failure:a && b",
        "command_failure:npm",
    ] {
        store
            .rules
            .push(make_rule(trigger, "action", RuleStatus::Active));
    }
    store.run_decay_cycle();
    for rule in &store.rules {
        if rule.trigger == "command_failure:npm" {
            assert!(rule.is_alive(), "npm rule should survive: {}", rule.trigger);
            assert_eq!(rule.revoked_reason, None);
        } else {
            assert_eq!(rule.status, RuleStatus::Dead, "{}", rule.trigger);
            assert_eq!(
                rule.revoked_reason.as_deref(),
                Some("disallowed command trigger (builtin/keyword/assignment)")
            );
        }
    }
}

#[test]
fn is_var_assignment_recognizes_append_form() {
    assert!(is_var_assignment("FOO=1"));
    assert!(is_var_assignment("FOO+=1"));
    assert!(is_var_assignment("_x=y"));
    assert!(is_var_assignment("PATH+=/usr/bin"));
    assert!(!is_var_assignment("1FOO=y"));
    assert!(!is_var_assignment("+=y"));
    assert!(!is_var_assignment("plain"));
    assert!(!is_var_assignment(""));
}

#[test]
fn split_command_segments_is_quote_aware() {
    // Delimiters inside quotes do not split: ONE segment.
    assert_eq!(
        split_command_segments("printf '%s' 'skip; python -V'"),
        vec!["printf '%s' 'skip; python -V'"]
    );
    assert_eq!(
        split_command_segments("echo \"a && b\""),
        vec!["echo \"a && b\""]
    );
    // Unquoted delimiters split. `&&` yields an empty segment between
    // the two ampersands; empty segments have no command word.
    assert_eq!(
        split_command_segments("cd /tmp && python x.py"),
        vec!["cd /tmp ", "", " python x.py"]
    );
    assert_eq!(
        split_command_segments("a;b|c&d\ne"),
        vec!["a", "b", "c", "d", "e"]
    );
    // Backslash-escaped delimiter does not split.
    assert_eq!(split_command_segments("echo a\\;b"), vec!["echo a\\;b"]);
    // Empty input still yields one empty segment.
    assert_eq!(split_command_segments(""), vec![""]);
}

#[test]
fn command_word_is_quote_aware() {
    // Quoted command word is unquoted.
    assert_eq!(command_word("'python' x.py").as_deref(), Some("python"));
    assert_eq!(command_word("\"python\" x.py").as_deref(), Some("python"));
    // Quoted value with spaces: the whole thing is one assignment word.
    assert_eq!(
        command_word("FOO='hello world' bar").as_deref(),
        Some("bar")
    );
    assert_eq!(
        command_word("BAR=\"a b c\" git status").as_deref(),
        Some("git")
    );
    // Append-assignment form is skipped.
    assert_eq!(command_word("PATH+=/x python").as_deref(), Some("python"));
    // Quoted segment never changes the command word.
    assert_eq!(
        command_word("printf '%s' 'skip; python -V'").as_deref(),
        Some("printf")
    );
    // Only assignments → None.
    assert_eq!(command_word("FOO=1"), None);
    assert_eq!(command_word(""), None);
    assert_eq!(command_word("   "), None);
}
#[test]
fn decay_cycle_never_revokes_non_command_failure_triggers() {
    // Only `command_failure:` triggers can be disallowed command
    // triggers. File paths may contain `=`; free-text and special
    // triggers must never be touched by the reclaim pass (GH-813).
    let mut store = RulesStore::default();
    for trigger in [
        "file_churn:config/foo=bar.toml",
        "multi_agent_start",
        "cd /tmp && echo hi",
        "build failure in CI",
    ] {
        store
            .rules
            .push(make_rule(trigger, "action", RuleStatus::Active));
    }
    store.run_decay_cycle();
    for rule in &store.rules {
        assert!(
            rule.is_alive(),
            "non-command_failure trigger wrongly revoked: {}",
            rule.trigger
        );
    }
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
    store.rules.push(make_rule("a", "b", RuleStatus::Active));
    store.rules.push(make_rule("c", "d", RuleStatus::Proposed));
    store.rules.push(make_rule("e", "f", RuleStatus::Dormant));
    store.rules.push(make_rule("g", "h", RuleStatus::Dead));
    let stats = store.stats();
    assert_eq!(stats.total, 4);
    assert_eq!(stats.active, 1);
    assert_eq!(stats.proposed, 1);
    assert_eq!(stats.dormant, 1);
    assert_eq!(stats.dead, 1);
}

#[test]
fn split_command_segments_backslash_in_single_quotes_is_literal() {
    let cmd = r"printf '%s\n' 'a\'; python -V";
    let segments = split_command_segments(cmd);
    assert_eq!(segments.len(), 2);
    assert_eq!(segments[0], r"printf '%s\n' 'a\'");
    assert_eq!(segments[1], " python -V");
    assert_eq!(command_word(segments[1]), Some("python".into()));
}

#[test]
fn trackable_and_command_word_with_quoted_command() {
    assert!(is_trackable_command(r#""python" -V"#));
    assert_eq!(command_word(r#""python" -V"#), Some("python".into()));
    assert!(!is_trackable_command(r#""echo" hi"#));
    assert!(!is_trackable_command("FOO=bar npm test"));
    assert_eq!(command_word("FOO=bar npm test"), Some("npm".into()));
}
