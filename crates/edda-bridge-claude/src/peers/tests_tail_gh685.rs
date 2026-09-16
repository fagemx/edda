use super::*;

/// §6.1 — a `request_delivered` marker is transport provenance, never an ack:
/// a delivered-but-unacked request is still live.
#[test]
fn request_delivered_is_not_an_ack() {
    let _store = crate::isolated_store();
    let pid = "test_gh685_delivered_not_ack";
    let _ = edda_store::ensure_dirs(pid);
    let _ = fs::remove_file(coordination_path(pid));

    assert!(write_request_with_id(
        pid, "s-alpha", "req-1", "alpha", "beta", "hi"
    ));
    write_request_delivered(pid, "req-1", "beta", "machine-a", "evt-1");

    let board = compute_board_state(pid);
    assert_eq!(
        board.request_delivered.len(),
        1,
        "the delivered marker is folded"
    );
    assert_eq!(board.request_delivered[0].request_id, "req-1");
    assert_eq!(board.request_delivered[0].via_machine, "machine-a");
    assert!(board.request_acks.is_empty(), "delivered is not an ack");

    let (live, expired) = partition_requests_for_session(&board, "s-alpha", "beta");
    assert_eq!(
        live.len(),
        1,
        "a delivered-but-unacked request is still live"
    );
    assert_eq!(live[0].id, "req-1");
    assert!(expired.is_empty());

    let _ = fs::remove_dir_all(edda_store::project_dir(pid));
}

/// §6.10 — the board folds delivered and acked as distinct axes.
#[test]
fn board_folding_keeps_delivered_and_acked_distinct() {
    let _store = crate::isolated_store();
    let pid = "test_gh685_distinct";
    let _ = edda_store::ensure_dirs(pid);
    let _ = fs::remove_file(coordination_path(pid));

    assert!(write_request_with_id(
        pid, "s-alpha", "req-9", "alpha", "beta", "hi"
    ));
    write_request_delivered(pid, "req-9", "beta", "machine-a", "evt-9");
    let board = compute_board_state(pid);
    assert_eq!(board.request_delivered.len(), 1);
    assert!(board.request_acks.is_empty());

    write_request_ack_id(pid, "s-beta", "alpha", "req-9").expect("ack");
    let board = compute_board_state(pid);
    assert_eq!(
        board.request_delivered.len(),
        1,
        "the delivered marker survives the ack"
    );
    assert_eq!(board.request_acks.len(), 1, "the ack is folded");
    assert_eq!(
        board.request_acks[0].request_ids.as_deref(),
        Some(["req-9".to_string()].as_slice())
    );
    let (live, _expired) = partition_requests_for_session(&board, "s-beta", "beta");
    assert!(live.is_empty(), "the ack retires the request");

    // An unknown id is a named error, not a silent success.
    let error = write_request_ack_id(pid, "s-beta", "alpha", "req-missing")
        .expect_err("unknown id must be refused");
    assert!(error.to_string().contains("unknown request id"));

    // A repeated envelope with the same request id is refused, not applied twice.
    assert!(!write_request_with_id(
        pid, "s-alpha", "req-9", "alpha", "beta", "hi"
    ));
    assert!(write_remote_request(
        pid,
        "node-machine-b",
        "req-9",
        "alpha",
        "beta",
        "hi",
        "machine-b",
    )
    .is_err());
    assert_eq!(
        compute_board_state(pid).requests.len(),
        1,
        "a redelivered request id is not applied twice"
    );

    let _ = fs::remove_dir_all(edda_store::project_dir(pid));
}
