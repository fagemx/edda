use std::process::Command;

#[test]
fn bundle_help_shows_the_review_deprecation_before_command_execution() {
    let output = Command::new(env!("CARGO_BIN_EXE_edda"))
        .args(["bundle", "--help"])
        .output()
        .expect("spawn edda bundle --help");

    assert!(output.status.success(), "stderr={:?}", output.stderr);
    let help = String::from_utf8_lossy(&output.stdout);
    assert!(help.contains("Deprecated: use `edda review`"));
    assert!(help.contains("independent SHA-pinned reviews"));
}
