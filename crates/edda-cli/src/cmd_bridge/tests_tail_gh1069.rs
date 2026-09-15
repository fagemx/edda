//! Claim-field tests for `edda peers --json` (GH-1069), kept out of
//! `cmd_bridge/tests.rs` because that file is at its GH-779 ceiling. This
//! module is wired from `tests_tail_gh671.rs`, which `tests.rs` already
//! includes.
use super::super::peers_json;

#[test]
fn peers_json_claims_distinguish_freshness_from_blocking() {
    // GH-1069: `stale` answers "is this claim fresh?" against the heartbeat
    // window; `blocks` must answer "does it still refuse a writer?" against
    // the claim TTL. Only a claim between the two windows tells them apart:
    // the other fixtures sit on the same side of both.
    let _store = crate::test_support::isolated_store();
    let repo = tempfile::tempdir().expect("tempdir");
    let pid = edda_store::project_id(repo.path());
    let _ = edda_store::ensure_dirs(&pid);

    // Past the heartbeat window (120s), inside the claim TTL (24h): not
    // fresh, but still standing for a bare-CLI claimant.
    crate::test_support::write_aged_claim(&pid, "cli-an-hour", 3600, &["src/an-hour.rs".into()]);
    // Past the TTL: neither fresh nor standing.
    crate::test_support::write_aged_claim(
        &pid,
        "cli-two-months",
        60 * 60 * 24 * 60,
        &["src/two-months.rs".into()],
    );
    // A live heartbeat makes a fresh claim judgeable: fresh and standing.
    edda_bridge_claude::peers::write_heartbeat_minimal(&pid, "live-session", "live", ".");
    edda_bridge_claude::peers::write_claim(&pid, "live-session", "live", &["src/live.rs".into()]);

    let json = peers_json(&pid, &edda_store::project_root(repo.path()));
    let claims = json["claims"].as_array().expect("claims array");
    let by_label = |label: &str| -> &serde_json::Value {
        claims
            .iter()
            .find(|c| c["label"] == label)
            .unwrap_or_else(|| panic!("claim {label} missing: {json}"))
    };

    let an_hour = by_label("cli-an-hour");
    assert_eq!(
        an_hour["stale"], true,
        "an hour is past the heartbeat window: {an_hour}"
    );
    assert_eq!(
        an_hour["blocks"], true,
        "an hour is inside the claim TTL, so it still refuses a writer: {an_hour}"
    );

    let two_months = by_label("cli-two-months");
    assert_eq!(two_months["stale"], true, "{two_months}");
    assert_eq!(
        two_months["blocks"], false,
        "past the claim TTL nothing keeps it standing: {two_months}"
    );

    let live = by_label("live");
    assert_eq!(live["stale"], false, "{live}");
    assert_eq!(live["blocks"], true, "{live}");
}
