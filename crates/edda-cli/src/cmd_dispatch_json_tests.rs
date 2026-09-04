#[test]
fn json_shape_pins_field_names_for_each_outcome() {
    let cases = vec![
        (
            PhaseResult::AgentDone {
                cost_usd: Some(1.25),
                result_text: Some("did it".into()),
            },
            "done",
        ),
        (
            PhaseResult::AgentCrash {
                error: "boom".into(),
            },
            "crash",
        ),
        (PhaseResult::Timeout, "timeout"),
        (PhaseResult::MaxTurns { cost_usd: None }, "max_turns"),
        (
            PhaseResult::BudgetExceeded { cost_usd: None },
            "budget_exceeded",
        ),
    ];
    for (result, expected_outcome) in cases {
        let out = DispatchOutput::from_result(
            result,
            "sess-1".into(),
            "inherited".into(),
            "unknown".into(),
            "unknown".into(),
        );
        let value: serde_json::Value =
            serde_json::from_str(&out.to_json()).expect("json parses");
        assert_eq!(value["outcome"].as_str(), Some(expected_outcome));
        assert!(value["result_text"].is_null() || value["result_text"].is_string());
        assert!(value["cost_usd"].is_null() || value["cost_usd"].is_number());
        assert_eq!(value["session_id"].as_str(), Some("sess-1"));
        assert!(value["error"].is_null() || value["error"].is_string());
        let mut keys: Vec<&str> = value
            .as_object()
            .expect("json object")
            .keys()
            .map(|k| k.as_str())
            .collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            vec![
                "cost_usd",
                "elapsed_measured",
                "elapsed_ms",
                "error",
                "model_observed",
                "model_requested",
                "outcome",
                "result_text",
                "session_id",
                "session_observed"
            ]
        );
    }
}
