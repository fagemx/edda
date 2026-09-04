use super::*;

#[test]
fn estimate_cost_sonnet() {
    let usage = UsageSnapshot {
        model: "claude-sonnet-4-20250514".into(),
        input_tokens: 1_000_000,
        output_tokens: 100_000,
        ..Default::default()
    };
    let cost = estimate_cost(&usage).expect("sonnet should be priceable");
    // input: 1M * $3/M = $3.00, output: 0.1M * $15/M = $1.50 -> $4.50
    assert!((cost - 4.50).abs() < 0.01, "cost={cost}");
}

#[test]
fn estimate_cost_opus() {
    let usage = UsageSnapshot {
        model: "claude-opus-5-20250514".into(),
        input_tokens: 500_000,
        output_tokens: 50_000,
        ..Default::default()
    };
    let cost = estimate_cost(&usage).expect("opus should be priceable");
    // Opus 5 rates: input 5/M, output 25/M
    // input: 0.5M * $5/M = $2.50, output: 0.05M * $25/M = $1.25 -> $3.75
    assert!((cost - 3.75).abs() < 0.01, "cost={cost}");
}

#[test]
fn estimate_cost_haiku() {
    let usage = UsageSnapshot {
        model: "claude-haiku-4-5".into(),
        input_tokens: 1_000_000,
        output_tokens: 100_000,
        ..Default::default()
    };
    let cost = estimate_cost(&usage).expect("haiku should be priceable");
    // Haiku 4.5 rates: input 1/M, output 5/M
    // input: 1M * $1/M = $1.00, output: 0.1M * $5/M = $0.50 -> $1.50
    assert!((cost - 1.50).abs() < 0.01, "cost={cost}");
}

#[test]
fn estimate_cost_fable_and_mythos_cache_multiplier() {
    let usage_fable = UsageSnapshot {
        model: "claude-fable-5-1".into(),
        // 1M total input, 600k are cache-read, 100k are cache-create
        input_tokens: 1_000_000,
        output_tokens: 100_000,
        cache_read_tokens: 600_000,
        cache_creation_tokens: 100_000,
        ..Default::default()
    };
    let cost_fable = estimate_cost(&usage_fable).expect("fable should be priceable");
    // full-price input: (1M - 600k - 100k) = 300k * $10/M = $3.00
    // cache-read: 600k * $10/M * 0.025 = $0.15
    // cache-create: 100k * $10/M * 1.25 = $1.25
    // output: 100k * $50/M = $5.00
    // total = $9.40
    assert!((cost_fable - 9.40).abs() < 0.01, "cost={cost_fable}");

    let usage_mythos = UsageSnapshot {
        model: "claude-mythos-5-1".into(),
        input_tokens: 1_000_000,
        output_tokens: 100_000,
        ..Default::default()
    };
    let cost_mythos = estimate_cost(&usage_mythos).expect("mythos should be priceable");
    // input: 1M * $10/M = $10.00, output: 0.1M * $50/M = $5.00 -> $15.00
    assert!((cost_mythos - 15.00).abs() < 0.01, "cost={cost_mythos}");
}

#[test]
fn estimate_cost_cache_aware() {
    let usage = UsageSnapshot {
        model: "claude-sonnet-4-20250514".into(),
        // 1M total input, 600k are cache-read, 100k are cache-create
        input_tokens: 1_000_000,
        output_tokens: 100_000,
        cache_read_tokens: 600_000,
        cache_creation_tokens: 100_000,
        ..Default::default()
    };
    let cost = estimate_cost(&usage).expect("sonnet should be priceable");
    // full-price input: (1M - 600k - 100k) = 300k * $3/M = $0.90
    // cache-read: 600k * $3/M * 0.1 = $0.18
    // cache-create: 100k * $3/M * 1.25 = $0.375
    // output: 100k * $15/M = $1.50
    // total ≈ $2.955
    assert!((cost - 2.955).abs() < 0.01, "cost={cost}");
}

#[test]
fn estimate_cost_unknown_model() {
    let usage = UsageSnapshot {
        model: "gpt-4o".into(),
        input_tokens: 1_000_000,
        output_tokens: 100_000,
        ..Default::default()
    };
    let cost = estimate_cost(&usage);
    assert_eq!(cost, None, "unknown model should return None");
}

#[test]
fn lookup_pricing_fable_and_mythos_return_rates() {
    let fable = lookup_pricing("claude-fable-5-1").expect("fable pricing");
    assert_eq!(fable.input_per_m, 10.0);
    assert_eq!(fable.output_per_m, 50.0);
    assert_eq!(fable.cache_read_multiplier, 0.025);

    let mythos = lookup_pricing("claude-mythos-5-1").expect("mythos pricing");
    assert_eq!(mythos.input_per_m, 10.0);
    assert_eq!(mythos.output_per_m, 50.0);
    assert_eq!(mythos.cache_read_multiplier, 0.025);
}

#[test]
fn lookup_pricing_env_override_supported() {
    crate::with_env_guard(
        &[("EDDA_MODEL_PRICING", Some("custom-model:2.0:8.0:0.05"))],
        || {
            let pricing = lookup_pricing("custom-model-v1").expect("custom model pricing");
            assert_eq!(pricing.input_per_m, 2.0);
            assert_eq!(pricing.output_per_m, 8.0);
            assert_eq!(pricing.cache_read_multiplier, 0.05);
        },
    );
}

#[test]
fn signals_extract_usage_from_transcript() {
    let _store = crate::isolated_store();
    let records = vec![
        serde_json::json!({
            "type": "system",
            "model": "claude-sonnet-4-20250514"
        }),
        serde_json::json!({
            "type": "assistant",
            "message": {
                "role": "assistant",
                "model": "claude-sonnet-4-20250514",
                "usage": {
                    "input_tokens": 1000,
                    "output_tokens": 500,
                    "cache_read_input_tokens": 200,
                    "cache_creation_input_tokens": 50
                },
                "content": [{ "type": "text", "text": "Hello" }]
            }
        }),
        serde_json::json!({
            "type": "assistant",
            "message": {
                "role": "assistant",
                "usage": {
                    "input_tokens": 2000,
                    "output_tokens": 800,
                    "cache_read_input_tokens": 100,
                    "cache_creation_input_tokens": 0
                },
                "content": [{ "type": "text", "text": "World" }]
            }
        }),
    ];
    let path = make_transcript(&records);
    let signals = extract_session_signals(&path);
    assert_eq!(signals.usage.model, "claude-sonnet-4-20250514");
    assert_eq!(signals.usage.input_tokens, 3000);
    assert_eq!(signals.usage.output_tokens, 1300);
    assert_eq!(signals.usage.cache_read_tokens, 300);
    assert_eq!(signals.usage.cache_creation_tokens, 50);
    assert_eq!(signals.usage.total_tokens(), 4300);
    assert!(
        signals.usage.usage_observed,
        "usage records were present in the transcript"
    );
    let _ = fs::remove_file(&path);
}

#[test]
fn signals_usage_presence_recorded_independently_of_counters() {
    let _store = crate::isolated_store();
    // GH-585 round 2 P1-1: presence must not be inferred from the token
    // counters. A usage record with all-zero tokens still sets the flag.
    let records = vec![serde_json::json!({
        "type": "assistant",
        "message": {
            "role": "assistant",
            "model": "claude-sonnet-4-20250514",
            "usage": {
                "input_tokens": 0,
                "output_tokens": 0,
                "cache_read_input_tokens": 0,
                "cache_creation_input_tokens": 0
            },
            "content": [{ "type": "text", "text": "Hello" }]
        }
    })];
    let path = make_transcript(&records);
    let signals = extract_session_signals(&path);
    assert_eq!(signals.usage.input_tokens, 0);
    assert_eq!(signals.usage.output_tokens, 0);
    assert!(
        signals.usage.usage_observed,
        "a usage record with all-zero counters is still a usage observation"
    );
    let _ = fs::remove_file(&path);
}

#[test]
fn signals_no_usage_record_means_no_presence() {
    let _store = crate::isolated_store();
    let records = vec![
        serde_json::json!({
            "type": "system",
            "model": "claude-sonnet-4-20250514"
        }),
        serde_json::json!({
            "type": "assistant",
            "message": {
                "role": "assistant",
                "model": "claude-sonnet-4-20250514",
                "content": [{ "type": "text", "text": "Hello" }]
            }
        }),
    ];
    let path = make_transcript(&records);
    let signals = extract_session_signals(&path);
    assert!(
        !signals.usage.usage_observed,
        "no message.usage record in the transcript"
    );
    let _ = fs::remove_file(&path);
}

#[test]
fn usage_save_and_read_round_trip() {
    let _store = crate::isolated_store();
    let pid = "test_usage_rt_00";
    let _ = edda_store::ensure_dirs(pid);

    let signals = SessionSignals {
        usage: UsageSnapshot {
            model: "claude-sonnet-4-20250514".into(),
            input_tokens: 5000,
            output_tokens: 2000,
            cache_read_tokens: 100,
            cache_creation_tokens: 50,
            usage_observed: true,
            ..Default::default()
        },
        ..Default::default()
    };
    save_session_signals(pid, "test-session", &signals);

    let loaded = read_usage_state(pid);
    assert_eq!(loaded.model, "claude-sonnet-4-20250514");
    assert_eq!(loaded.input_tokens, 5000);
    assert_eq!(loaded.output_tokens, 2000);
    assert_eq!(loaded.cache_read_tokens, 100);
    assert_eq!(loaded.cache_creation_tokens, 50);
    assert!(loaded.usage_observed, "presence must survive usage.json");

    let _ = fs::remove_dir_all(edda_store::project_dir(pid));
}

#[test]
fn usage_state_round_trips_measured_zero_presence() {
    let _store = crate::isolated_store();
    // GH-585 round 2 P1-1: a usage snapshot with usage_observed=true and
    // all-zero counters must survive usage.json unchanged — that is the
    // measured-zero case the magnitude-based check used to lose.
    let pid = "test_usage_zero_rt";
    let _ = edda_store::ensure_dirs(pid);

    let signals = SessionSignals {
        usage: UsageSnapshot {
            model: "claude-sonnet-4-20250514".into(),
            usage_observed: true,
            ..Default::default()
        },
        ..Default::default()
    };
    save_session_signals(pid, "test-session", &signals);

    let loaded = read_usage_state(pid);
    assert!(loaded.usage_observed);
    assert_eq!(loaded.input_tokens, 0);
    assert_eq!(loaded.output_tokens, 0);

    let _ = fs::remove_dir_all(edda_store::project_dir(pid));
}
