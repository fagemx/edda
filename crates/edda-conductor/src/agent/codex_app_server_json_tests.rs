#[test]
fn correlates_response_ids_while_skipping_notifications() -> anyhow::Result<()> {
    let lines = [
        r#"{"method":"thread/started","params":{"thread":{"id":"t-1"}}}"#,
        r#"{"id":1,"result":{"ignored":true}}"#,
        r#"{"id":2,"result":{"thread":{"id":"t-1"}}}"#,
    ];

    let result = response_for_id(lines.iter().copied(), 2)?;

    assert_eq!(result["thread"]["id"], "t-1");
    Ok(())
}

#[test]
fn rejects_malformed_app_server_json() {
    let error = response_for_id(["not-json"], 2).expect_err("malformed JSON should fail");

    assert!(error.to_string().contains("invalid app-server JSON"));
}

#[test]
fn preserves_json_rpc_error_message() {
    let error = response_for_id(
        [r#"{"id":2,"error":{"code":-32602,"message":"bad params"}}"#],
        2,
    )
    .expect_err("JSON-RPC error should fail");

    assert!(error.to_string().contains("bad params"));
}
