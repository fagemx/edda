/// `edda peers` renders `label@machine`; an unidentified session keeps its
/// marker, and a missing machine identity degrades to the bare label.
#[test]
fn peer_display_label_carries_the_machine_suffix() {
    use super::peers::peer_display_label;
    use edda_bridge_claude::peers::PeerSummary;

    let peer = |label: &str| PeerSummary {
        session_id: "s".into(),
        label: label.into(),
        age_secs: 5,
        is_live: true,
        last_heartbeat: String::new(),
        focus_files: vec![],
        task_subjects: vec![],
        files_modified_count: 0,
        recent_commits: vec![],
        claimed_paths: vec![],
        claimed_subject: None,
        branch: None,
        current_phase: None,
    };

    assert_eq!(
        peer_display_label(&peer("main"), Some("docs")),
        "main@docs",
        "the render is label@machine"
    );
    assert_eq!(
        peer_display_label(&peer("main"), None),
        "main",
        "no machine identity: bare label, no guess"
    );
    assert_eq!(
        peer_display_label(&peer(""), Some("docs")),
        "(no label)",
        "the unidentified marker never gains a machine suffix"
    );
}
