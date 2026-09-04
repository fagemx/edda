//! GH-603: exercise the real CLI, not just serde accepting unknown fields.
use std::process::Command;

#[test]
fn carrier_examples_pass_dry_run_and_fail_before_execution() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    for shape in ["coding", "research", "loop"] {
        let fixture = root.join(format!("docs/design/infra-contracts/{shape}.yaml"));
        let dir = tempfile::tempdir().unwrap();
        let output = Command::new(env!("CARGO_BIN_EXE_edda"))
            .current_dir(dir.path())
            .args(["conduct", "run"])
            .arg(&fixture)
            .arg("--dry-run")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(String::from_utf8_lossy(&output.stderr).contains("Schema preview only"));
        assert!(String::from_utf8_lossy(&output.stdout).contains("[dry-run] Plan:"));
        let output = Command::new(env!("CARGO_BIN_EXE_edda"))
            .current_dir(dir.path())
            .args(["conduct", "run"])
            .arg(&fixture)
            .output()
            .unwrap();
        assert!(!output.status.success());
        assert!(String::from_utf8_lossy(&output.stderr).contains("preview"));
    }
}
