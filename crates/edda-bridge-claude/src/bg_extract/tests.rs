use super::*;

#[test]
fn test_parse_llm_output_valid_json() {
    let _store = crate::isolated_store();
    let input = r#"[
            {
                "key": "db.engine",
                "value": "sqlite",
                "reason": "embedded, zero-config",
                "confidence": 0.92,
                "evidence": "用 SQLite 就好"
            },
            {
                "key": "auth.method",
                "value": "JWT",
                "reason": "stateless",
                "confidence": 0.85,
                "evidence": "用 JWT RS256"
            }
        ]"#;

    let decisions = parse_llm_decisions(input);
    assert_eq!(decisions.len(), 2);
    assert_eq!(decisions[0].key, "db.engine");
    assert_eq!(decisions[0].value, "sqlite");
    assert_eq!(
        decisions[0].reason.as_deref(),
        Some("embedded, zero-config")
    );
    assert!((decisions[0].confidence - 0.92).abs() < 0.001);
    assert_eq!(decisions[1].key, "auth.method");
}

#[test]
fn test_parse_llm_output_markdown_wrapped() {
    let _store = crate::isolated_store();
    let input = r#"Here are the decisions I found:

```json
[{"key": "api.framework", "value": "axum", "reason": "async Rust", "confidence": 0.9, "evidence": "chose axum"}]
```

That's it."#;

    let decisions = parse_llm_decisions(input);
    assert_eq!(decisions.len(), 1);
    assert_eq!(decisions[0].key, "api.framework");
}

#[test]
fn test_parse_llm_output_empty_array() {
    let _store = crate::isolated_store();
    let decisions = parse_llm_decisions("[]");
    assert!(decisions.is_empty());
}

#[test]
fn test_parse_llm_output_garbage() {
    let _store = crate::isolated_store();
    let decisions = parse_llm_decisions("I couldn't find any decisions.");
    assert!(decisions.is_empty());
}

#[test]
fn test_parse_llm_output_missing_fields() {
    let _store = crate::isolated_store();
    let input = r#"[{"key": "db", "value": "pg"}]"#;
    let decisions = parse_llm_decisions(input);
    assert_eq!(decisions.len(), 1);
    assert_eq!(decisions[0].key, "db");
    assert!(decisions[0].reason.is_none());
    assert!((decisions[0].confidence - 0.5).abs() < 0.001);
}

#[test]
fn test_truncate_text_within_limit() {
    let text = "short text";
    assert_eq!(truncate_text(text, 100), "short text");
}

#[test]
fn test_truncate_text_over_limit() {
    let text = "line1\nline2\nline3\nline4\nline5";
    let result = truncate_text(text, 15);
    assert!(result.contains("[... transcript truncated ...]"));
    assert!(result.contains("line5"));
}

#[test]
fn test_extract_json_array_bare() {
    assert_eq!(extract_json_array("  [1,2,3]  "), "[1,2,3]");
}

#[test]
fn test_extract_json_array_in_codeblock() {
    let input = "```json\n[1,2]\n```";
    assert_eq!(extract_json_array(input), "[1,2]");
}

#[test]
fn test_daily_cost_tracking() {
    let today = today_date();
    let cost = DailyCost {
        date: today.clone(),
        total_usd: 0.10,
        calls: 5,
    };
    let json = serde_json::to_string_pretty(&cost).unwrap();
    let loaded: DailyCost = serde_json::from_str(&json).unwrap();
    assert_eq!(loaded.date, today);
    assert!((loaded.total_usd - 0.10).abs() < 0.001);
    assert_eq!(loaded.calls, 5);
}

#[test]
fn test_draft_status_serde() {
    let draft = ExtractedDecision {
        key: "test.key".to_string(),
        value: "test_value".to_string(),
        reason: Some("because".to_string()),
        confidence: 0.8,
        evidence: "evidence".to_string(),
        source_turn: 5,
        status: DraftStatus::Pending,
        kind: DecisionKind::Extraction,
        original_reason: None,
    };

    let json = serde_json::to_string(&draft).unwrap();
    let parsed: ExtractedDecision = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed.status, DraftStatus::Pending);
    assert_eq!(parsed.key, "test.key");
    assert_eq!(parsed.kind, DecisionKind::Extraction);
    assert!(parsed.original_reason.is_none());
}

#[test]
fn test_extraction_state_serde() {
    let state = ExtractionState {
        status: "completed".to_string(),
        extracted_at: "2026-03-11T10:00:00Z".to_string(),
        transcript_hash: "blake3:abc123".to_string(),
        decisions_count: 3,
    };

    let json = serde_json::to_string_pretty(&state).unwrap();
    let parsed: ExtractionState = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed.status, "completed");
    assert_eq!(parsed.decisions_count, 3);
}

#[test]
fn test_build_extraction_prompt() {
    let _store = crate::isolated_store();
    let prompt = build_extraction_prompt("test transcript", &[]);
    assert!(prompt.contains("決策提取器"));
    assert!(prompt.contains("test transcript"));
    assert!(prompt.contains("JSON"));
    // No enhancement section when no vague decisions
    assert!(!prompt.contains("增強模糊"));
}

#[test]
fn test_extract_text_from_content_string() {
    let _store = crate::isolated_store();
    let content = serde_json::json!("hello world");
    assert_eq!(extract_text_from_content(&content), "hello world");
}

#[test]
fn test_extract_text_from_content_blocks() {
    let _store = crate::isolated_store();
    let content = serde_json::json!([
        {"type": "text", "text": "part 1"},
        {"type": "tool_use", "name": "grep"},
        {"type": "text", "text": "part 2"}
    ]);
    let result = extract_text_from_content(&content);
    assert!(result.contains("part 1"));
    assert!(result.contains("part 2"));
    assert!(!result.contains("grep"));
}

// ── Decision Reason Quality Enhancement Tests (#194) ──

#[test]
fn test_is_vague_reason_none() {
    assert!(is_vague_reason(None));
}

#[test]
fn test_is_vague_reason_short() {
    assert!(is_vague_reason(Some("ok")));
    assert!(is_vague_reason(Some("yes")));
    assert!(is_vague_reason(Some("   short   "))); // trimmed < 15
}

#[test]
fn test_is_vague_reason_exact_match() {
    assert!(is_vague_reason(Some("for now")));
    assert!(is_vague_reason(Some("just")));
    assert!(is_vague_reason(Some("simple")));
    assert!(is_vague_reason(Some("easier")));
    assert!(is_vague_reason(Some("because")));
    assert!(is_vague_reason(Some("for now.")));
    assert!(is_vague_reason(Some("暫時")));
    assert!(is_vague_reason(Some("先這樣")));
    assert!(is_vague_reason(Some("好了")));
    assert!(is_vague_reason(Some("方便")));
}

#[test]
fn test_is_vague_reason_good() {
    assert!(!is_vague_reason(Some("embedded, zero-config for MVP")));
    assert!(!is_vague_reason(Some("stateless, scales horizontally")));
}

#[test]
fn test_is_vague_reason_threshold() {
    // Exactly 15 chars → not vague
    assert!(!is_vague_reason(Some("123456789012345")));
    // 14 chars → vague (too short)
    assert!(is_vague_reason(Some("12345678901234")));
}

#[test]
fn test_is_vague_reason_contains_vague_word_but_longer() {
    // "just" appears as substring but the whole reason is long and specific
    assert!(!is_vague_reason(Some(
        "just because it supports async well and is production-ready"
    )));
}

#[test]
fn test_decision_kind_serde() {
    // Round-trip for Extraction
    let json = serde_json::to_string(&DecisionKind::Extraction).unwrap();
    assert_eq!(json, r#""extraction""#);
    let parsed: DecisionKind = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed, DecisionKind::Extraction);

    // Round-trip for Enhancement
    let json = serde_json::to_string(&DecisionKind::Enhancement).unwrap();
    assert_eq!(json, r#""enhancement""#);
    let parsed: DecisionKind = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed, DecisionKind::Enhancement);
}

#[test]
fn test_backward_compat_no_kind() {
    // Existing JSON without `kind` or `original_reason` should deserialize
    let json = r#"{
            "key": "db.engine",
            "value": "sqlite",
            "reason": "embedded",
            "confidence": 0.9,
            "evidence": "some quote",
            "source_turn": 3,
            "status": "pending"
        }"#;
    let parsed: ExtractedDecision = serde_json::from_str(json).unwrap();
    assert_eq!(parsed.kind, DecisionKind::Extraction);
    assert!(parsed.original_reason.is_none());
}

#[test]
fn test_parse_enhancement_output() {
    let _store = crate::isolated_store();
    let input = r#"[{
            "kind": "enhancement",
            "key": "db.engine",
            "value": "sqlite",
            "original_reason": "for now",
            "reason": "SQLite chosen for MVP — embedded, zero external deps",
            "confidence": 0.85,
            "evidence": "用戶說先用 SQLite"
        }]"#;

    let decisions = parse_llm_decisions(input);
    assert_eq!(decisions.len(), 1);
    assert_eq!(decisions[0].kind, DecisionKind::Enhancement);
    assert_eq!(decisions[0].original_reason.as_deref(), Some("for now"));
    assert!(decisions[0].reason.as_ref().unwrap().contains("SQLite"));
}

#[test]
fn test_parse_mixed_output() {
    let _store = crate::isolated_store();
    let input = r#"[
            {
                "kind": "extraction",
                "key": "api.framework",
                "value": "axum",
                "reason": "async Rust",
                "confidence": 0.9,
                "evidence": "chose axum"
            },
            {
                "kind": "enhancement",
                "key": "db.engine",
                "value": "sqlite",
                "original_reason": "for now",
                "reason": "embedded, zero-config for MVP phase",
                "confidence": 0.85,
                "evidence": "discussed sqlite"
            }
        ]"#;

    let decisions = parse_llm_decisions(input);
    assert_eq!(decisions.len(), 2);
    assert_eq!(decisions[0].kind, DecisionKind::Extraction);
    assert!(decisions[0].original_reason.is_none());
    assert_eq!(decisions[1].kind, DecisionKind::Enhancement);
    assert_eq!(decisions[1].original_reason.as_deref(), Some("for now"));
}

#[test]
fn test_prompt_with_vague_decisions() {
    let _store = crate::isolated_store();
    let vague = vec![
        RecordedDecision {
            key: "db.engine".to_string(),
            value: "sqlite".to_string(),
            reason: Some("for now".to_string()),
        },
        RecordedDecision {
            key: "auth.method".to_string(),
            value: "JWT".to_string(),
            reason: None,
        },
    ];
    let prompt = build_extraction_prompt("test transcript", &vague);
    assert!(prompt.contains("增強模糊"));
    assert!(prompt.contains("db.engine"));
    assert!(prompt.contains("auth.method"));
    assert!(prompt.contains("(none)"));
    assert!(prompt.contains("enhancement"));
}

#[test]
fn test_prompt_without_vague_decisions() {
    let _store = crate::isolated_store();
    let prompt = build_extraction_prompt("test transcript", &[]);
    assert!(!prompt.contains("增強模糊"));
    assert!(!prompt.contains("enhancement"));
}

#[test]
fn test_parse_edda_decide_command_double_quoted() {
    let cmd = r#"edda decide "db.engine=sqlite" --reason "embedded, zero-config""#;
    let d = parse_edda_decide_command(cmd).unwrap();
    assert_eq!(d.key, "db.engine");
    assert_eq!(d.value, "sqlite");
    assert_eq!(d.reason.as_deref(), Some("embedded, zero-config"));
}

#[test]
fn test_parse_edda_decide_command_single_quoted() {
    let cmd = "edda decide 'auth.method=JWT' --reason 'stateless'";
    let d = parse_edda_decide_command(cmd).unwrap();
    assert_eq!(d.key, "auth.method");
    assert_eq!(d.value, "JWT");
    assert_eq!(d.reason.as_deref(), Some("stateless"));
}

#[test]
fn test_parse_edda_decide_command_no_reason() {
    let cmd = r#"edda decide "cache.strategy=redis""#;
    let d = parse_edda_decide_command(cmd).unwrap();
    assert_eq!(d.key, "cache.strategy");
    assert_eq!(d.value, "redis");
    assert!(d.reason.is_none());
}

#[test]
fn test_parse_edda_decide_command_not_found() {
    let cmd = "cargo build --release";
    assert!(parse_edda_decide_command(cmd).is_none());
}

#[test]
fn test_extract_recorded_decisions_from_transcript() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("test.jsonl");

    // Write a transcript with edda decide commands
    let lines = [
        r#"{"type":"assistant","message":{"content":"Let me record this."},"tool_input":{"command":"edda decide \"db.engine=sqlite\" --reason \"for now\""}}"#,
        r#"{"type":"human","message":{"content":"ok"}}"#,
        r#"{"type":"assistant","message":{"content":"Another decision."},"tool_input":{"command":"edda decide \"auth.method=JWT\""}}"#,
    ];
    fs::write(&path, lines.join("\n")).unwrap();

    let decisions = extract_recorded_decisions_from_transcript(&path);
    assert_eq!(decisions.len(), 2);
    assert_eq!(decisions[0].key, "db.engine");
    assert_eq!(decisions[0].value, "sqlite");
    assert_eq!(decisions[0].reason.as_deref(), Some("for now"));
    assert_eq!(decisions[1].key, "auth.method");
    assert_eq!(decisions[1].value, "JWT");
    assert!(decisions[1].reason.is_none());
}
