use super::*;

fn dv(key: &str, value: &str, authority: &str, ts: &str) -> edda_ledger::view::DecisionView {
    edda_ledger::view::DecisionView {
        event_id: format!("evt_{key}"),
        branch: "main".into(),
        ts: Some(ts.into()),
        key: key.into(),
        value: value.into(),
        reason: String::new(),
        domain: key.split('.').next().unwrap_or(key).into(),
        status: "active".into(),
        authority: authority.into(),
        reversibility: "medium".into(),
        affected_paths: vec![],
        tags: vec![],
        propagation: "local".into(),
        supersedes_id: None,
        review_after: None,
        village_id: None,
    }
}

#[test]
fn two_tier_splits_ratified_from_unratified() {
    let _store = crate::isolated_store();
    let decisions = vec![
        dv("db.engine", "postgres", "operator", "2026-07-14T00:00:00Z"),
        dv("api.style", "REST", "agent", "2026-07-14T00:00:00Z"),
    ];
    let ratified: std::collections::BTreeSet<String> = ["evt_db.engine".to_string()].into();

    let out = render_decisions_two_tier(&decisions, &ratified).unwrap();
    // Ratified section names the binding key; unratified section names the other.
    assert!(out.contains("Operator-ratified"));
    assert!(out.contains("db.engine"));
    assert!(out.contains("Unratified"));
    assert!(out.contains("api.style"));
    // The binding key appears above the unratified header.
    let ratified_pos = out.find("db.engine").unwrap();
    let unratified_hdr = out.find("Unratified").unwrap();
    assert!(ratified_pos < unratified_hdr, "ratified must render first");
}

#[test]
fn two_tier_annotates_unratified_authorship() {
    let _store = crate::isolated_store();
    let decisions = vec![
        dv("a.b", "1", "agent", "2026-07-14T00:00:00Z"),
        dv("c.d", "2", "human", "2026-07-14T00:00:00Z"),
    ];
    let out = render_decisions_two_tier(&decisions, &std::collections::BTreeSet::new()).unwrap();
    assert!(
        !out.contains("Operator-ratified"),
        "no ratified section expected"
    );
    assert!(out.contains("[agent]"));
    assert!(out.contains("[human]"));
}

#[test]
fn two_tier_legacy_unratified_all_in_unratified_tier() {
    let _store = crate::isolated_store();
    // Pre-401 decisions default to authority=human but have no ratify
    // event — they must land in the unratified tier, never binding.
    let decisions = vec![dv("legacy.key", "v", "human", "2026-01-01T00:00:00Z")];
    let out = render_decisions_two_tier(&decisions, &std::collections::BTreeSet::new()).unwrap();
    assert!(!out.contains("Operator-ratified"));
    assert!(out.contains("Unratified"));
    assert!(out.contains("legacy.key"));
}

#[test]
fn two_tier_empty_returns_none() {
    let _store = crate::isolated_store();
    assert!(render_decisions_two_tier(&[], &std::collections::BTreeSet::new()).is_none());
}

#[test]
fn two_tier_all_ratified_omits_unratified_section() {
    let _store = crate::isolated_store();
    let decisions = vec![dv("k", "v", "operator", "2026-07-14T00:00:00Z")];
    let ratified: std::collections::BTreeSet<String> = ["evt_k".to_string()].into();
    let out = render_decisions_two_tier(&decisions, &ratified).unwrap();
    assert!(out.contains("Operator-ratified"));
    assert!(!out.contains("Unratified"));
}

#[test]
fn capability_gap_serde_roundtrip() {
    let gap = CapabilityGap {
        title: "Missing retry logic".to_string(),
        category: "reliability".to_string(),
        severity: "medium".to_string(),
        description: "API calls lack retry logic".to_string(),
        evidence: vec!["bg_extract.rs".to_string()],
        suggested_labels: vec!["enhancement".to_string()],
        confidence: 0.75,
        status: GapStatus::Pending,
    };
    let json = serde_json::to_string(&gap).unwrap();
    let parsed: CapabilityGap = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed.title, "Missing retry logic");
    assert_eq!(parsed.confidence, 0.75);
    assert_eq!(parsed.status, GapStatus::Pending);
}

#[test]
fn scan_result_serde_roundtrip() {
    let result = ScanResult {
        scan_id: "scan_abc123".to_string(),
        scanned_at: "2026-03-12T10:00:00Z".to_string(),
        gaps: vec![CapabilityGap {
            title: "Test gap".to_string(),
            category: "testing".to_string(),
            severity: "low".to_string(),
            description: "Needs more tests".to_string(),
            evidence: vec![],
            suggested_labels: vec![],
            confidence: 0.8,
            status: GapStatus::Pending,
        }],
        model: "claude-3-5-haiku-20241022".to_string(),
        input_tokens: 1000,
        output_tokens: 500,
        cost_usd: 0.0035,
        codebase_hash: "blake3:abc".to_string(),
    };
    let json = serde_json::to_string(&result).unwrap();
    let parsed: ScanResult = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed.scan_id, "scan_abc123");
    assert_eq!(parsed.gaps.len(), 1);
    assert_eq!(parsed.cost_usd, 0.0035);
}

#[test]
fn scan_state_serde_roundtrip() {
    let state = ScanState {
        last_scan_at: "2026-03-12T10:00:00Z".to_string(),
        codebase_hash: "blake3:abc".to_string(),
        gaps_found: 3,
        status: "completed".to_string(),
    };
    let json = serde_json::to_string(&state).unwrap();
    let parsed: ScanState = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed.last_scan_at, "2026-03-12T10:00:00Z");
    assert_eq!(parsed.gaps_found, 3);
}

#[test]
fn parse_scan_response_valid_json() {
    let _store = crate::isolated_store();
    let input = r#"[
            {
                "title": "Missing error handling",
                "category": "reliability",
                "severity": "high",
                "description": "No retry logic in API calls",
                "evidence": ["bg_extract.rs"],
                "suggested_labels": ["bug"],
                "confidence": 0.9
            }
        ]"#;
    let gaps = parse_scan_response(input);
    assert_eq!(gaps.len(), 1);
    assert_eq!(gaps[0].title, "Missing error handling");
    assert_eq!(gaps[0].confidence, 0.9);
}

#[test]
fn parse_scan_response_markdown_fenced() {
    let _store = crate::isolated_store();
    let input = r#"Here are the gaps:
```json
[
    {
        "title": "Missing tests",
        "category": "testing",
        "severity": "medium",
        "description": "No unit tests",
        "evidence": [],
        "suggested_labels": [],
        "confidence": 0.7
    }
]
```"#;
    let gaps = parse_scan_response(input);
    assert_eq!(gaps.len(), 1);
    assert_eq!(gaps[0].title, "Missing tests");
}

#[test]
fn parse_scan_response_malformed_returns_empty() {
    let _store = crate::isolated_store();
    let input = "This is not JSON at all";
    let gaps = parse_scan_response(input);
    assert!(gaps.is_empty());
}

#[test]
fn parse_scan_response_embedded_json_array() {
    let _store = crate::isolated_store();
    let input = r#"Analysis complete. Found gaps:
[{"title":"Gap 1","category":"feature","severity":"low","description":"desc","evidence":[],"suggested_labels":[],"confidence":0.6}]
End of analysis."#;
    let gaps = parse_scan_response(input);
    assert_eq!(gaps.len(), 1);
    assert_eq!(gaps[0].title, "Gap 1");
}

#[test]
fn build_scan_prompt_includes_snapshot() {
    let prompt = build_scan_prompt("test snapshot data");
    assert!(prompt.contains("test snapshot data"));
    assert!(prompt.contains("capability gaps"));
    assert!(prompt.contains("JSON array"));
}

#[test]
fn should_run_returns_false_when_disabled() {
    let _store = crate::isolated_store();
    crate::with_env_guard(
        &[
            ("EDDA_BG_ENABLED", Some("0")),
            ("EDDA_LLM_API_KEY", Some("test-key")),
        ],
        || {
            assert!(!should_run("test_scan_disabled"));
        },
    );
}

#[test]
fn should_run_returns_false_without_api_key() {
    let _store = crate::isolated_store();
    crate::with_env_guard(
        &[("EDDA_BG_ENABLED", Some("1")), ("EDDA_LLM_API_KEY", None)],
        || {
            assert!(!should_run("test_scan_no_key"));
        },
    );
}

#[test]
fn should_run_returns_false_within_cooldown() {
    let _store = crate::isolated_store();
    let pid = "test_scan_cooldown_check";
    let _ = edda_store::ensure_dirs(pid);

    // Write a recent scan state
    let state = ScanState {
        last_scan_at: now_rfc3339(), // Just now
        codebase_hash: "blake3:test".to_string(),
        gaps_found: 0,
        status: "completed".to_string(),
    };
    let path = scan_state_path(pid);
    let _ = fs::create_dir_all(path.parent().unwrap());
    let _ = fs::write(&path, serde_json::to_string_pretty(&state).unwrap());

    crate::with_env_guard(
        &[
            ("EDDA_BG_ENABLED", Some("1")),
            ("EDDA_LLM_API_KEY", Some("test-key")),
            ("EDDA_SCAN_COOLDOWN_DAYS", Some("7")),
        ],
        || {
            assert!(!should_run(pid));
        },
    );

    // Cleanup
    let _ = fs::remove_dir_all(edda_store::project_dir(pid));
}

#[test]
fn cooldown_respects_env_override() {
    let _store = crate::isolated_store();
    let pid = "test_scan_cooldown_override";
    let _ = edda_store::ensure_dirs(pid);

    // Write a scan state from 2 days ago
    let two_days_ago = time::OffsetDateTime::now_utc() - time::Duration::days(2);
    let ts = two_days_ago
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap();
    let state = ScanState {
        last_scan_at: ts,
        codebase_hash: "blake3:old".to_string(),
        gaps_found: 1,
        status: "completed".to_string(),
    };
    let path = scan_state_path(pid);
    let _ = fs::create_dir_all(path.parent().unwrap());
    let _ = fs::write(&path, serde_json::to_string_pretty(&state).unwrap());

    crate::with_env_guard(&[("EDDA_SCAN_COOLDOWN_DAYS", Some("7"))], || {
        // With 7-day cooldown, should NOT have elapsed
        assert!(!cooldown_elapsed(pid));
    });

    crate::with_env_guard(&[("EDDA_SCAN_COOLDOWN_DAYS", Some("1"))], || {
        // With 1-day cooldown, SHOULD have elapsed
        assert!(cooldown_elapsed(pid));
    });

    // Cleanup
    let _ = fs::remove_dir_all(edda_store::project_dir(pid));
}

#[test]
fn draft_storage_roundtrip() {
    let _store = crate::isolated_store();
    let pid = "test_scan_drafts";
    let _ = edda_store::ensure_dirs(pid);

    let result = ScanResult {
        scan_id: "scan_test123".to_string(),
        scanned_at: "2026-03-12T10:00:00Z".to_string(),
        gaps: vec![
            CapabilityGap {
                title: "Gap A".to_string(),
                category: "testing".to_string(),
                severity: "medium".to_string(),
                description: "Missing tests".to_string(),
                evidence: vec!["file.rs".to_string()],
                suggested_labels: vec!["test".to_string()],
                confidence: 0.8,
                status: GapStatus::Pending,
            },
            CapabilityGap {
                title: "Gap B".to_string(),
                category: "docs".to_string(),
                severity: "low".to_string(),
                description: "Missing docs".to_string(),
                evidence: vec![],
                suggested_labels: vec![],
                confidence: 0.6,
                status: GapStatus::Pending,
            },
        ],
        model: "test-model".to_string(),
        input_tokens: 100,
        output_tokens: 50,
        cost_usd: 0.001,
        codebase_hash: "blake3:test".to_string(),
    };

    save_scan_drafts(pid, &result).unwrap();

    let scans = list_pending_scans(pid).unwrap();
    assert_eq!(scans.len(), 1);
    assert_eq!(scans[0].gaps.len(), 2);

    // Dismiss one gap
    dismiss_gap(pid, "scan_test123", 0).unwrap();
    let scans = list_pending_scans(pid).unwrap();
    assert_eq!(scans.len(), 1); // Still has 1 pending gap
    assert_eq!(scans[0].gaps[0].status, GapStatus::Dismissed);
    assert_eq!(scans[0].gaps[1].status, GapStatus::Pending);

    // Accept the other gap
    let gap = accept_gap(pid, "scan_test123", 1).unwrap();
    assert_eq!(gap.title, "Gap B");

    // No more pending gaps
    let scans = list_pending_scans(pid).unwrap();
    assert!(scans.is_empty());

    // Cleanup
    let _ = fs::remove_dir_all(edda_store::project_dir(pid));
}

#[test]
fn audit_log_appends() {
    let _store = crate::isolated_store();
    let pid = "test_scan_audit";
    let _ = edda_store::ensure_dirs(pid);
    // Start from a known state (GH-415), as bg_detect's copy of this test
    // already does. Appending to a log in the real store while asserting an
    // exact count means a run that panicked before its cleanup leaves rows
    // for this one to count — which panics, skips cleanup, and leaves more.
    // Once red on a machine, red forever.
    let _ = fs::remove_file(audit_log_path(pid));

    let entry = AuditEntry {
        ts: "2026-03-12T10:00:00Z".to_string(),
        scan_id: "scan_1".to_string(),
        gaps_found: 3,
        cost_usd: 0.02,
        model: "test-model".to_string(),
        status: "completed".to_string(),
    };
    append_audit_log(pid, &entry).unwrap();

    let path = audit_log_path(pid);
    let content = fs::read_to_string(&path).unwrap();
    assert!(content.contains("scan_1"));
    assert_eq!(content.lines().count(), 1);

    // Append another
    let entry2 = AuditEntry {
        ts: "2026-03-12T11:00:00Z".to_string(),
        scan_id: "scan_2".to_string(),
        gaps_found: 1,
        cost_usd: 0.01,
        model: "test-model".to_string(),
        status: "completed".to_string(),
    };
    append_audit_log(pid, &entry2).unwrap();
    let content2 = fs::read_to_string(&path).unwrap();
    assert_eq!(content2.lines().count(), 2);

    // Cleanup
    let _ = fs::remove_dir_all(edda_store::project_dir(pid));
}

#[test]
fn collect_crate_inventory_parses_workspace() {
    let _store = crate::isolated_store();
    // Create a temp workspace
    let dir = tempfile::tempdir().unwrap();
    let cargo = dir.path().join("Cargo.toml");
    fs::write(
        &cargo,
        r#"[workspace]
members = [
    "crates/edda-core",
    "crates/edda-store",
]
"#,
    )
    .unwrap();

    let result = collect_crate_inventory(dir.path().to_str().unwrap());
    assert!(result.is_some());
    let text = result.unwrap();
    assert!(text.contains("crates/edda-core"));
    assert!(text.contains("crates/edda-store"));
}

#[test]
fn gap_status_default_is_pending() {
    let gap: CapabilityGap = serde_json::from_str(
        r#"{
                "title": "test",
                "category": "test",
                "severity": "low",
                "description": "test",
                "evidence": [],
                "suggested_labels": [],
                "confidence": 0.5
            }"#,
    )
    .unwrap();
    assert_eq!(gap.status, GapStatus::Pending);
}

#[test]
fn snapshot_assembly_with_nonexistent_cwd() {
    let _store = crate::isolated_store();
    let result = assemble_project_snapshot("/nonexistent/path/xyz", "test_proj");
    assert!(result.is_err());
}
