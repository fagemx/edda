use super::*;

#[test]
fn signal_kind_serde_roundtrip() {
    let signal = RawSignal {
        kind: SignalKind::FailurePattern,
        severity: "high".to_string(),
        summary: "test failure".to_string(),
        evidence: vec!["session_1".to_string()],
        metric_value: 5.0,
        baseline_value: 3.0,
        confidence: 0.8,
    };
    let json = serde_json::to_string(&signal).unwrap();
    let parsed: RawSignal = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed.kind, SignalKind::FailurePattern);
    assert_eq!(parsed.severity, "high");
    assert_eq!(parsed.confidence, 0.8);
}

#[test]
fn detect_result_serde_roundtrip() {
    let result = DetectResult {
        detect_id: "detect_abc123".to_string(),
        detected_at: "2026-03-12T10:00:00Z".to_string(),
        raw_signals: vec![RawSignal {
            kind: SignalKind::CostAnomaly,
            severity: "medium".to_string(),
            summary: "Cost spike".to_string(),
            evidence: vec![],
            metric_value: 0.50,
            baseline_value: 0.10,
            confidence: 0.85,
        }],
        patterns: vec![DetectedPattern {
            signals: vec![],
            correlation: "standalone".to_string(),
            suggested_action: "Review spending".to_string(),
            created_at: "2026-03-12T10:00:00Z".to_string(),
        }],
        model: Some("claude-3-5-haiku-20241022".to_string()),
        input_tokens: 500,
        output_tokens: 200,
        cost_usd: 0.0015,
    };
    let json = serde_json::to_string(&result).unwrap();
    let parsed: DetectResult = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed.detect_id, "detect_abc123");
    assert_eq!(parsed.raw_signals.len(), 1);
    assert_eq!(parsed.patterns.len(), 1);
    assert_eq!(parsed.cost_usd, 0.0015);
}

#[test]
fn detect_state_serde_roundtrip() {
    let state = DetectState {
        last_detect_at: "2026-03-12T10:00:00Z".to_string(),
        sessions_since_last: 5,
        status: "completed".to_string(),
    };
    let json = serde_json::to_string(&state).unwrap();
    let parsed: DetectState = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed.sessions_since_last, 5);
    assert_eq!(parsed.status, "completed");
}

#[test]
fn should_run_returns_false_when_disabled() {
    let _store = crate::isolated_store();
    crate::with_env_guard(&[("EDDA_BG_ENABLED", Some("0"))], || {
        assert!(!should_run("test_detect_disabled"));
    });
}

#[test]
fn should_run_returns_true_when_never_run() {
    let _store = crate::isolated_store();
    let pid = "test_detect_never_run";
    // Ensure no state file exists
    let _ = fs::remove_file(detect_state_path(pid));

    crate::with_env_guard(&[("EDDA_BG_ENABLED", Some("1"))], || {
        assert!(should_run(pid));
    });
}

#[test]
fn should_run_returns_false_below_interval() {
    let _store = crate::isolated_store();
    let pid = "test_detect_below_interval";
    let _ = edda_store::ensure_dirs(pid);

    let state = DetectState {
        last_detect_at: now_rfc3339(),
        sessions_since_last: 2, // Below default 10
        status: "completed".to_string(),
    };
    save_detect_state_raw(pid, &state).unwrap();

    crate::with_env_guard(
        &[
            ("EDDA_BG_ENABLED", Some("1")),
            ("EDDA_DETECT_INTERVAL", Some("10")),
        ],
        || {
            assert!(!should_run(pid));
        },
    );

    // Cleanup
    let _ = fs::remove_dir_all(edda_store::project_dir(pid));
}

#[test]
fn should_run_returns_false_within_cooldown() {
    let _store = crate::isolated_store();
    let pid = "test_detect_cooldown";
    let _ = edda_store::ensure_dirs(pid);

    let state = DetectState {
        last_detect_at: now_rfc3339(), // Just now
        sessions_since_last: 100,      // Well above threshold
        status: "completed".to_string(),
    };
    save_detect_state_raw(pid, &state).unwrap();

    crate::with_env_guard(
        &[
            ("EDDA_BG_ENABLED", Some("1")),
            ("EDDA_DETECT_INTERVAL", Some("1")),
            ("EDDA_DETECT_COOLDOWN_HOURS", Some("24")),
        ],
        || {
            assert!(!should_run(pid));
        },
    );

    // Cleanup
    let _ = fs::remove_dir_all(edda_store::project_dir(pid));
}

#[test]
fn state_persistence_roundtrip() {
    let _store = crate::isolated_store();
    let pid = "test_detect_state_persist";
    let _ = edda_store::ensure_dirs(pid);

    let state = DetectState {
        last_detect_at: "2026-03-12T10:00:00Z".to_string(),
        sessions_since_last: 7,
        status: "completed".to_string(),
    };
    save_detect_state_raw(pid, &state).unwrap();

    let loaded = load_detect_state(pid).unwrap();
    assert_eq!(loaded.last_detect_at, "2026-03-12T10:00:00Z");
    assert_eq!(loaded.sessions_since_last, 7);

    // Cleanup
    let _ = fs::remove_dir_all(edda_store::project_dir(pid));
}

#[test]
fn increment_session_count_works() {
    let _store = crate::isolated_store();
    let pid = "test_detect_increment";
    let _ = edda_store::ensure_dirs(pid);

    // Start fresh
    let _ = fs::remove_file(detect_state_path(pid));

    increment_session_count(pid, "sess-test");
    let state = load_detect_state(pid).unwrap();
    assert_eq!(state.sessions_since_last, 1);

    increment_session_count(pid, "sess-test");
    let state = load_detect_state(pid).unwrap();
    assert_eq!(state.sessions_since_last, 2);

    // Cleanup
    let _ = fs::remove_dir_all(edda_store::project_dir(pid));
}

#[test]
fn audit_log_appends() {
    let _store = crate::isolated_store();
    let pid = "test_detect_audit";
    let _ = edda_store::ensure_dirs(pid);
    let _ = fs::remove_file(audit_log_path(pid));

    let entry = AuditEntry {
        ts: "2026-03-12T10:00:00Z".to_string(),
        detect_id: "detect_1".to_string(),
        signals_found: 2,
        patterns_found: 1,
        cost_usd: 0.001,
        model: Some("test-model".to_string()),
        status: "completed".to_string(),
    };
    append_audit_log(pid, &entry).unwrap();

    let path = audit_log_path(pid);
    let content = fs::read_to_string(&path).unwrap();
    assert!(content.contains("detect_1"));
    assert_eq!(content.lines().count(), 1);

    // Append another
    let entry2 = AuditEntry {
        ts: "2026-03-12T11:00:00Z".to_string(),
        detect_id: "detect_2".to_string(),
        signals_found: 0,
        patterns_found: 0,
        cost_usd: 0.0,
        model: None,
        status: "completed".to_string(),
    };
    append_audit_log(pid, &entry2).unwrap();
    let content2 = fs::read_to_string(&path).unwrap();
    assert_eq!(content2.lines().count(), 2);

    // Cleanup
    let _ = fs::remove_dir_all(edda_store::project_dir(pid));
}

#[test]
fn promote_raw_signals_creates_patterns() {
    let signals = vec![
        RawSignal {
            kind: SignalKind::FailurePattern,
            severity: "high".to_string(),
            summary: "Recurring bash failures".to_string(),
            evidence: vec!["s1".to_string()],
            metric_value: 5.0,
            baseline_value: 3.0,
            confidence: 0.8,
        },
        RawSignal {
            kind: SignalKind::CostAnomaly,
            severity: "medium".to_string(),
            summary: "Cost spike".to_string(),
            evidence: vec![],
            metric_value: 0.5,
            baseline_value: 0.1,
            confidence: 0.85,
        },
    ];

    let patterns = promote_raw_signals(&signals);
    assert_eq!(patterns.len(), 2);
    assert_eq!(patterns[0].correlation, "standalone");
    assert!(patterns[0].suggested_action.contains("Recurring bash"));
    assert!(patterns[1].suggested_action.contains("Cost spike"));
}

#[test]
fn parse_detect_response_valid_json() {
    let _store = crate::isolated_store();
    let signals = vec![RawSignal {
        kind: SignalKind::FailurePattern,
        severity: "high".to_string(),
        summary: "test".to_string(),
        evidence: vec![],
        metric_value: 1.0,
        baseline_value: 0.5,
        confidence: 0.8,
    }];

    let response = r#"[
            {
                "correlation": "Failures causing cost increase",
                "suggested_action": "Add retry logic",
                "signal_indices": [0]
            }
        ]"#;
    let patterns = parse_detect_response(response, &signals);
    assert_eq!(patterns.len(), 1);
    assert_eq!(patterns[0].correlation, "Failures causing cost increase");
    assert_eq!(patterns[0].suggested_action, "Add retry logic");
    assert_eq!(patterns[0].signals.len(), 1);
}

#[test]
fn parse_detect_response_embedded_json() {
    let _store = crate::isolated_store();
    let signals = vec![RawSignal {
        kind: SignalKind::CostAnomaly,
        severity: "medium".to_string(),
        summary: "test".to_string(),
        evidence: vec![],
        metric_value: 1.0,
        baseline_value: 0.5,
        confidence: 0.9,
    }];

    let response = r#"Here are my findings:
[{"correlation": "standalone", "suggested_action": "Review costs", "signal_indices": [0]}]
End."#;
    let patterns = parse_detect_response(response, &signals);
    assert_eq!(patterns.len(), 1);
    assert_eq!(patterns[0].suggested_action, "Review costs");
}

#[test]
fn parse_detect_response_malformed_returns_empty() {
    let _store = crate::isolated_store();
    let signals = vec![];
    let patterns = parse_detect_response("not json at all", &signals);
    assert!(patterns.is_empty());
}

#[test]
fn build_detect_prompt_includes_context() {
    let prompt = build_detect_prompt("test anomaly data");
    assert!(prompt.contains("test anomaly data"));
    assert!(prompt.contains("anomaly signals"));
    assert!(prompt.contains("JSON array"));
}

#[test]
fn detect_cost_anomalies_needs_min_data() {
    let _store = crate::isolated_store();
    let pid = "test_detect_cost_min";
    let _ = edda_store::ensure_dirs(pid);

    // With no audit files, should return empty
    let signals = detect_cost_anomalies(pid);
    assert!(signals.is_empty());

    // Cleanup
    let _ = fs::remove_dir_all(edda_store::project_dir(pid));
}

#[test]
fn detect_cost_anomalies_flags_spike() {
    let _store = crate::isolated_store();
    let pid = "test_detect_cost_spike";
    let _ = edda_store::ensure_dirs(pid);

    // Create synthetic audit log with a cost spike
    let dir = edda_store::project_dir(pid).join("state");
    fs::create_dir_all(&dir).unwrap();
    let audit_path = dir.join("bg_extract_audit.jsonl");

    let mut lines = Vec::new();
    // 7 days of normal cost ($0.01/day)
    for day in 1..=7 {
        lines.push(format!(
            r#"{{"ts":"2026-03-{:02}T10:00:00Z","cost_usd":0.01,"status":"completed"}}"#,
            day
        ));
    }
    // Day 8: big spike ($0.10)
    lines.push(r#"{"ts":"2026-03-08T10:00:00Z","cost_usd":0.10,"status":"completed"}"#.to_string());

    fs::write(&audit_path, lines.join("\n")).unwrap();

    let signals = detect_cost_anomalies(pid);
    assert_eq!(signals.len(), 1);
    assert_eq!(signals[0].kind, SignalKind::CostAnomaly);
    assert!(signals[0].metric_value > signals[0].baseline_value);

    // Cleanup
    let _ = fs::remove_dir_all(edda_store::project_dir(pid));
}

#[test]
fn detect_quality_degradation_empty_data() {
    let _store = crate::isolated_store();
    let pid = "test_detect_quality_empty";
    let _ = edda_store::ensure_dirs(pid);

    let signals = detect_quality_degradation(pid);
    assert!(signals.is_empty());

    // Cleanup
    let _ = fs::remove_dir_all(edda_store::project_dir(pid));
}

#[test]
fn detect_quality_degradation_flags_drop() {
    let _store = crate::isolated_store();
    let pid = "test_detect_quality_drop";
    let _ = edda_store::ensure_dirs(pid);

    let dir = edda_store::project_dir(pid).join("state");
    fs::create_dir_all(&dir).unwrap();
    let audit_path = dir.join("bg_digest_audit.jsonl");

    let mut lines = Vec::new();
    // 5 older successful sessions
    for i in 1..=5 {
        lines.push(format!(
            r#"{{"ts":"2026-03-0{i}T10:00:00Z","session_id":"s{i}","status":"completed"}}"#
        ));
    }
    // 5 recent failing sessions
    for i in 6..=10 {
        let d = if i <= 9 {
            format!("0{i}")
        } else {
            format!("{i}")
        };
        lines.push(format!(
            r#"{{"ts":"2026-03-{d}T10:00:00Z","session_id":"s{i}","status":"error"}}"#
        ));
    }

    fs::write(&audit_path, lines.join("\n")).unwrap();

    let signals = detect_quality_degradation(pid);
    assert_eq!(signals.len(), 1);
    assert_eq!(signals[0].kind, SignalKind::QualityDegradation);
    assert_eq!(signals[0].severity, "high"); // 100% drop

    // Cleanup
    let _ = fs::remove_dir_all(edda_store::project_dir(pid));
}

#[test]
fn detect_result_storage_roundtrip() {
    let _store = crate::isolated_store();
    let pid = "test_detect_result_store";
    let _ = edda_store::ensure_dirs(pid);

    let result = DetectResult {
        detect_id: "detect_test1".to_string(),
        detected_at: "2026-03-12T10:00:00Z".to_string(),
        raw_signals: vec![RawSignal {
            kind: SignalKind::FailurePattern,
            severity: "high".to_string(),
            summary: "test".to_string(),
            evidence: vec![],
            metric_value: 5.0,
            baseline_value: 3.0,
            confidence: 0.8,
        }],
        patterns: vec![],
        model: None,
        input_tokens: 0,
        output_tokens: 0,
        cost_usd: 0.0,
    };

    save_detect_result(pid, &result).unwrap();

    let path = detect_results_dir(pid).join("detect_test1.json");
    assert!(path.exists());
    let content = fs::read_to_string(&path).unwrap();
    let loaded: DetectResult = serde_json::from_str(&content).unwrap();
    assert_eq!(loaded.detect_id, "detect_test1");
    assert_eq!(loaded.raw_signals.len(), 1);

    // Cleanup
    let _ = fs::remove_dir_all(edda_store::project_dir(pid));
}

#[test]
fn cooldown_expired_allows_run() {
    let _store = crate::isolated_store();
    let state = DetectState {
        last_detect_at: "2020-01-01T00:00:00Z".to_string(), // Long ago
        sessions_since_last: 100,
        status: "completed".to_string(),
    };
    assert!(cooldown_elapsed(&state));
}

#[test]
fn cooldown_not_expired_blocks_run() {
    let _store = crate::isolated_store();
    let state = DetectState {
        last_detect_at: now_rfc3339(), // Just now
        sessions_since_last: 100,
        status: "completed".to_string(),
    };
    crate::with_env_guard(&[("EDDA_DETECT_COOLDOWN_HOURS", Some("24"))], || {
        assert!(!cooldown_elapsed(&state));
    });
}

#[test]
fn empty_last_detect_at_means_cooldown_elapsed() {
    let _store = crate::isolated_store();
    let state = DetectState {
        last_detect_at: String::new(),
        sessions_since_last: 0,
        status: "init".to_string(),
    };
    assert!(cooldown_elapsed(&state));
}
