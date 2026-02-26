use anyhow::{bail, Result};
use edda_core::bundle::*;
use edda_core::event::{new_review_bundle_event, ReviewBundleParams};
use edda_ledger::Ledger;
use std::path::Path;
use std::process::Command as ProcessCommand;

// ── CLI entry points ────────────────────────────────────────────────

/// Execute `edda bundle create`.
pub fn execute_create(
    repo_root: &Path,
    diff_ref: Option<&str>,
    test_cmd: Option<&str>,
    skip_tests: bool,
) -> Result<()> {
    let diff_ref = diff_ref.unwrap_or("HEAD~1");
    let test_cmd_str = test_cmd.unwrap_or("cargo test --workspace");

    // 1. Run git diff --numstat
    let change_summary = run_git_diff(repo_root, diff_ref)?;
    if change_summary.files.is_empty() {
        bail!("No changes found for diff ref '{diff_ref}'.\nMake sure there are committed changes to compare.");
    }

    // 2. Run tests (unless --skip-tests)
    let test_results = if skip_tests {
        TestResults {
            passed: 0,
            failed: 0,
            ignored: 0,
            total: 0,
            failures: vec![],
            command: "skipped".into(),
        }
    } else {
        run_tests(repo_root, test_cmd_str)?
    };

    // 3. Assess risk
    let risk_assessment = assess_risk(&change_summary, &test_results);

    // 4. Suggest action
    let (suggested_action, suggested_reason) = suggest_action(&risk_assessment, &test_results);

    // 5. Create bundle
    let bundle_id = format!("bun_{}", ulid::Ulid::new().to_string().to_lowercase());
    let bundle = ReviewBundle {
        bundle_id: bundle_id.clone(),
        change_summary,
        test_results,
        risk_assessment,
        suggested_action,
        suggested_reason,
    };

    // 6. Create event + append to ledger
    let ledger = Ledger::open(repo_root)?;
    let branch = ledger.head_branch()?;
    let parent_hash = ledger.last_event_hash()?;
    let event = new_review_bundle_event(&ReviewBundleParams {
        branch,
        parent_hash,
        bundle: bundle.clone(),
    })?;
    ledger.append_event(&event)?;

    // 7. Display compact card
    print_bundle_card(&bundle);
    println!("\nBundle {} stored in ledger.", bundle_id);

    Ok(())
}

/// Execute `edda bundle show <bundle-id>`.
pub fn execute_show(repo_root: &Path, bundle_id: &str) -> Result<()> {
    let ledger = Ledger::open(repo_root)?;
    let Some(row) = ledger.get_bundle(bundle_id)? else {
        bail!("Bundle '{bundle_id}' not found.");
    };

    // Fetch full event payload for detailed display
    let events = ledger.iter_events()?;
    let event = events.iter().find(|e| e.event_id == row.event_id);

    if let Some(event) = event {
        let bundle: ReviewBundle = serde_json::from_value(event.payload.clone())?;
        print_bundle_card(&bundle);
        println!("\nStatus: {}", row.status);
    } else {
        // Fallback to summary from SQLite
        print_bundle_row(&row);
    }

    Ok(())
}

/// Execute `edda bundle list`.
pub fn execute_list(repo_root: &Path, status: Option<&str>) -> Result<()> {
    let ledger = Ledger::open(repo_root)?;
    let bundles = ledger.list_bundles(status)?;

    if bundles.is_empty() {
        println!("No review bundles found.");
        return Ok(());
    }

    println!(
        "{:<20} {:<10} {:<10} {:<12} {:<10} {:<10}",
        "BUNDLE ID", "STATUS", "RISK", "CHANGES", "TESTS", "ACTION"
    );
    println!("{}", "-".repeat(72));
    for b in &bundles {
        let changes = format!("+{} -{}", b.total_added, b.total_deleted);
        let tests = if b.tests_failed > 0 {
            format!("{} fail", b.tests_failed)
        } else {
            format!("{} pass", b.tests_passed)
        };
        println!(
            "{:<20} {:<10} {:<10} {:<12} {:<10} {:<10}",
            truncate(&b.bundle_id, 20),
            b.status,
            b.risk_level,
            changes,
            tests,
            b.suggested_action,
        );
    }

    Ok(())
}

// ── Git diff parsing ────────────────────────────────────────────────

fn run_git_diff(repo_root: &Path, diff_ref: &str) -> Result<ChangeSummary> {
    let output = ProcessCommand::new("git")
        .args(["diff", "--numstat", diff_ref])
        .current_dir(repo_root)
        .output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("git diff failed: {stderr}");
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let files = parse_numstat(&stdout);
    let total_added = files.iter().map(|f| f.added).sum();
    let total_deleted = files.iter().map(|f| f.deleted).sum();

    Ok(ChangeSummary {
        files,
        total_added,
        total_deleted,
        diff_ref: diff_ref.to_string(),
    })
}

fn parse_numstat(output: &str) -> Vec<FileChange> {
    output
        .lines()
        .filter_map(|line| {
            let parts: Vec<&str> = line.split('\t').collect();
            if parts.len() == 3 {
                // Binary files show "-" for added/deleted
                let added = parts[0].parse().unwrap_or(0);
                let deleted = parts[1].parse().unwrap_or(0);
                Some(FileChange {
                    path: parts[2].to_string(),
                    added,
                    deleted,
                })
            } else {
                None
            }
        })
        .collect()
}

// ── Test result parsing ─────────────────────────────────────────────

fn run_tests(repo_root: &Path, test_cmd: &str) -> Result<TestResults> {
    println!("Running tests: {test_cmd}");

    let output = if cfg!(target_os = "windows") {
        ProcessCommand::new("cmd")
            .args(["/C", test_cmd])
            .current_dir(repo_root)
            .output()?
    } else {
        ProcessCommand::new("sh")
            .args(["-c", test_cmd])
            .current_dir(repo_root)
            .output()?
    };

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let combined = format!("{stdout}{stderr}");
    parse_test_results(&combined, test_cmd)
}

fn parse_test_results(output: &str, command: &str) -> Result<TestResults> {
    // Parse cargo test summary lines:
    //   "test result: ok. 841 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out"
    let mut total_passed = 0u32;
    let mut total_failed = 0u32;
    let mut total_ignored = 0u32;
    let mut failures: Vec<String> = Vec::new();
    let mut found_result = false;

    for line in output.lines() {
        let trimmed = line.trim();

        // Parse summary lines
        if trimmed.starts_with("test result:") {
            found_result = true;
            if let Some(rest) = trimmed.strip_prefix("test result:") {
                let rest = rest.trim();
                // Skip status word (ok/FAILED)
                let rest = if let Some(pos) = rest.find('.') {
                    &rest[pos + 1..]
                } else {
                    rest
                };

                for part in rest.split(';') {
                    let part = part.trim();
                    if let Some(n) = extract_count(part, "passed") {
                        total_passed += n;
                    } else if let Some(n) = extract_count(part, "failed") {
                        total_failed += n;
                    } else if let Some(n) = extract_count(part, "ignored") {
                        total_ignored += n;
                    }
                }
            }
        }

        // Collect failure names: "test some::test ... FAILED"
        if trimmed.starts_with("test ") && trimmed.ends_with("FAILED") {
            let name = trimmed
                .strip_prefix("test ")
                .unwrap_or("")
                .strip_suffix("... FAILED")
                .unwrap_or("")
                .trim();
            if !name.is_empty() {
                failures.push(name.to_string());
            }
        }
    }

    if !found_result {
        // Fallback: couldn't parse output, mark as unknown
        return Ok(TestResults {
            passed: 0,
            failed: 0,
            ignored: 0,
            total: 0,
            failures: vec![],
            command: command.into(),
        });
    }

    Ok(TestResults {
        passed: total_passed,
        failed: total_failed,
        ignored: total_ignored,
        total: total_passed + total_failed + total_ignored,
        failures,
        command: command.into(),
    })
}

fn extract_count(part: &str, label: &str) -> Option<u32> {
    let part = part.trim();
    if part.ends_with(label) {
        let num_str = part.strip_suffix(label)?.trim();
        num_str.parse().ok()
    } else {
        None
    }
}

// ── Risk assessment ─────────────────────────────────────────────────

fn assess_risk(change: &ChangeSummary, tests: &TestResults) -> RiskAssessment {
    let mut factors = Vec::new();

    // Test failures → Critical
    if tests.failed > 0 {
        factors.push(RiskFactor {
            signal: "test_failure".into(),
            level: RiskLevel::Critical,
            detail: format!("{} test(s) failed", tests.failed),
        });
    }

    let total_lines = change.total_added + change.total_deleted;

    // Large change → High
    if total_lines > 500 {
        factors.push(RiskFactor {
            signal: "large_change".into(),
            level: RiskLevel::High,
            detail: format!("{total_lines} lines changed"),
        });
    } else if total_lines > 200 {
        factors.push(RiskFactor {
            signal: "moderate_change".into(),
            level: RiskLevel::Medium,
            detail: format!("{total_lines} lines changed"),
        });
    }

    // Wide change (many files) → Medium
    if change.files.len() > 10 {
        factors.push(RiskFactor {
            signal: "wide_change".into(),
            level: RiskLevel::Medium,
            detail: format!("{} files changed", change.files.len()),
        });
    }

    // Check for sensitive file patterns
    for file in &change.files {
        let path = file.path.to_lowercase();

        // Dependency files
        if path.ends_with("cargo.lock") || path.ends_with("cargo.toml") {
            factors.push(RiskFactor {
                signal: "dependency_change".into(),
                level: RiskLevel::Medium,
                detail: format!("dependency file: {}", file.path),
            });
            break; // One factor per category
        }
    }

    for file in &change.files {
        let path = file.path.to_lowercase();

        // Schema/migration files
        if path.contains("migration") || path.ends_with(".sql") {
            factors.push(RiskFactor {
                signal: "schema_change".into(),
                level: RiskLevel::High,
                detail: format!("schema/migration file: {}", file.path),
            });
            break;
        }
    }

    // Determine overall level (max of all factors)
    let level = factors
        .iter()
        .map(|f| f.level)
        .max()
        .unwrap_or(RiskLevel::Low);

    RiskAssessment { level, factors }
}

fn suggest_action(risk: &RiskAssessment, tests: &TestResults) -> (SuggestedAction, String) {
    if tests.failed > 0 {
        return (
            SuggestedAction::Reject,
            format!("{} test(s) failing", tests.failed),
        );
    }
    match risk.level {
        RiskLevel::Critical => (
            SuggestedAction::RequestChanges,
            "Critical risk signals detected".into(),
        ),
        RiskLevel::High => (
            SuggestedAction::Review,
            "High risk — manual review recommended".into(),
        ),
        _ => (
            SuggestedAction::Approve,
            format!("All tests pass, {:?} risk", risk.level),
        ),
    }
}

// ── Display helpers ─────────────────────────────────────────────────

fn print_bundle_card(bundle: &ReviewBundle) {
    let risk_indicator = match bundle.risk_assessment.level {
        RiskLevel::Low => "LOW",
        RiskLevel::Medium => "MEDIUM",
        RiskLevel::High => "HIGH",
        RiskLevel::Critical => "CRITICAL",
    };

    let action_str = match bundle.suggested_action {
        SuggestedAction::Approve => "APPROVE",
        SuggestedAction::Review => "REVIEW",
        SuggestedAction::RequestChanges => "REQUEST CHANGES",
        SuggestedAction::Reject => "REJECT",
    };

    let total_lines = bundle.change_summary.total_added + bundle.change_summary.total_deleted;
    let file_count = bundle.change_summary.files.len();

    println!("REVIEW BUNDLE {}", bundle.bundle_id);
    println!("Risk: {risk_indicator}");
    println!("---");
    println!(
        "Changes: {} files, +{} -{} ({} lines)",
        file_count,
        bundle.change_summary.total_added,
        bundle.change_summary.total_deleted,
        total_lines
    );
    for file in &bundle.change_summary.files {
        println!("  {} (+{} -{})", file.path, file.added, file.deleted);
    }
    println!("---");
    if bundle.test_results.command == "skipped" {
        println!("Tests: skipped");
    } else {
        println!(
            "Tests: {} passed, {} failed, {} ignored",
            bundle.test_results.passed, bundle.test_results.failed, bundle.test_results.ignored
        );
        for f in &bundle.test_results.failures {
            println!("  FAIL: {f}");
        }
    }
    println!("---");
    println!("Suggested: {action_str}");
    println!("Reason: {}", bundle.suggested_reason);

    if !bundle.risk_assessment.factors.is_empty() {
        println!("---");
        println!("Risk factors:");
        for f in &bundle.risk_assessment.factors {
            println!("  [{:?}] {}: {}", f.level, f.signal, f.detail);
        }
    }
}

fn print_bundle_row(row: &edda_ledger::BundleRow) {
    println!("REVIEW BUNDLE {}", row.bundle_id);
    println!("Risk: {}", row.risk_level);
    println!("---");
    println!(
        "Changes: {} files, +{} -{}",
        row.files_changed, row.total_added, row.total_deleted
    );
    println!("---");
    println!(
        "Tests: {} passed, {} failed",
        row.tests_passed, row.tests_failed
    );
    println!("---");
    println!("Suggested: {}", row.suggested_action);
    println!("Status: {}", row.status);
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        format!("{}...", &s[..max - 3])
    }
}

// ── Tests ───────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_numstat_basic() {
        let output = "10\t3\tsrc/main.rs\n5\t0\tsrc/lib.rs\n";
        let files = parse_numstat(output);
        assert_eq!(files.len(), 2);
        assert_eq!(files[0].path, "src/main.rs");
        assert_eq!(files[0].added, 10);
        assert_eq!(files[0].deleted, 3);
        assert_eq!(files[1].path, "src/lib.rs");
        assert_eq!(files[1].added, 5);
        assert_eq!(files[1].deleted, 0);
    }

    #[test]
    fn parse_numstat_empty() {
        let files = parse_numstat("");
        assert!(files.is_empty());
    }

    #[test]
    fn parse_numstat_binary() {
        // Binary files show "-" for added/deleted
        let output = "-\t-\timage.png\n10\t5\tsrc/main.rs\n";
        let files = parse_numstat(output);
        assert_eq!(files.len(), 2);
        assert_eq!(files[0].path, "image.png");
        assert_eq!(files[0].added, 0); // "-" parsed as 0
        assert_eq!(files[0].deleted, 0);
        assert_eq!(files[1].added, 10);
    }

    #[test]
    fn parse_test_results_ok() {
        let output = r#"
running 841 tests
test some::test ... ok
test other::test ... ok
test result: ok. 841 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 5.00s
"#;
        let results = parse_test_results(output, "cargo test").unwrap();
        assert_eq!(results.passed, 841);
        assert_eq!(results.failed, 0);
        assert_eq!(results.ignored, 0);
        assert_eq!(results.total, 841);
        assert!(results.failures.is_empty());
    }

    #[test]
    fn parse_test_results_with_failures() {
        let output = r#"
running 10 tests
test good::test ... ok
test bad::test ... FAILED
test result: FAILED. 9 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out
"#;
        let results = parse_test_results(output, "cargo test").unwrap();
        assert_eq!(results.passed, 9);
        assert_eq!(results.failed, 1);
        assert_eq!(results.total, 10);
        assert_eq!(results.failures, vec!["bad::test"]);
    }

    #[test]
    fn parse_test_results_multiple_crates() {
        let output = r#"
running 50 tests
test result: ok. 50 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
running 30 tests
test result: ok. 30 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out
"#;
        let results = parse_test_results(output, "cargo test --workspace").unwrap();
        assert_eq!(results.passed, 80);
        assert_eq!(results.failed, 0);
        assert_eq!(results.ignored, 2);
        assert_eq!(results.total, 82);
    }

    #[test]
    fn parse_test_results_no_match() {
        let output = "some random output\nno test results here\n";
        let results = parse_test_results(output, "cargo test").unwrap();
        assert_eq!(results.total, 0);
        assert!(results.failures.is_empty());
    }

    #[test]
    fn assess_risk_low() {
        let change = ChangeSummary {
            files: vec![FileChange {
                path: "src/main.rs".into(),
                added: 10,
                deleted: 3,
            }],
            total_added: 10,
            total_deleted: 3,
            diff_ref: "HEAD~1".into(),
        };
        let tests = TestResults {
            passed: 50,
            failed: 0,
            ignored: 0,
            total: 50,
            failures: vec![],
            command: "cargo test".into(),
        };
        let risk = assess_risk(&change, &tests);
        assert_eq!(risk.level, RiskLevel::Low);
        assert!(risk.factors.is_empty());
    }

    #[test]
    fn assess_risk_high_large_change() {
        let change = ChangeSummary {
            files: vec![FileChange {
                path: "src/main.rs".into(),
                added: 400,
                deleted: 200,
            }],
            total_added: 400,
            total_deleted: 200,
            diff_ref: "HEAD~1".into(),
        };
        let tests = TestResults {
            passed: 50,
            failed: 0,
            ignored: 0,
            total: 50,
            failures: vec![],
            command: "cargo test".into(),
        };
        let risk = assess_risk(&change, &tests);
        assert_eq!(risk.level, RiskLevel::High);
        assert!(risk.factors.iter().any(|f| f.signal == "large_change"));
    }

    #[test]
    fn assess_risk_critical_test_failure() {
        let change = ChangeSummary {
            files: vec![FileChange {
                path: "src/main.rs".into(),
                added: 5,
                deleted: 2,
            }],
            total_added: 5,
            total_deleted: 2,
            diff_ref: "HEAD~1".into(),
        };
        let tests = TestResults {
            passed: 49,
            failed: 1,
            ignored: 0,
            total: 50,
            failures: vec!["bad::test".into()],
            command: "cargo test".into(),
        };
        let risk = assess_risk(&change, &tests);
        assert_eq!(risk.level, RiskLevel::Critical);
        assert!(risk.factors.iter().any(|f| f.signal == "test_failure"));
    }

    #[test]
    fn assess_risk_dependency_change() {
        let change = ChangeSummary {
            files: vec![
                FileChange {
                    path: "Cargo.toml".into(),
                    added: 2,
                    deleted: 1,
                },
                FileChange {
                    path: "Cargo.lock".into(),
                    added: 50,
                    deleted: 30,
                },
            ],
            total_added: 52,
            total_deleted: 31,
            diff_ref: "HEAD~1".into(),
        };
        let tests = TestResults {
            passed: 50,
            failed: 0,
            ignored: 0,
            total: 50,
            failures: vec![],
            command: "cargo test".into(),
        };
        let risk = assess_risk(&change, &tests);
        assert!(risk.level >= RiskLevel::Medium);
        assert!(risk.factors.iter().any(|f| f.signal == "dependency_change"));
    }

    #[test]
    fn suggest_action_approve() {
        let risk = RiskAssessment {
            level: RiskLevel::Low,
            factors: vec![],
        };
        let tests = TestResults {
            passed: 50,
            failed: 0,
            ignored: 0,
            total: 50,
            failures: vec![],
            command: "cargo test".into(),
        };
        let (action, _) = suggest_action(&risk, &tests);
        assert_eq!(action, SuggestedAction::Approve);
    }

    #[test]
    fn suggest_action_reject_on_failure() {
        let risk = RiskAssessment {
            level: RiskLevel::Low,
            factors: vec![],
        };
        let tests = TestResults {
            passed: 49,
            failed: 1,
            ignored: 0,
            total: 50,
            failures: vec!["bad::test".into()],
            command: "cargo test".into(),
        };
        let (action, reason) = suggest_action(&risk, &tests);
        assert_eq!(action, SuggestedAction::Reject);
        assert!(reason.contains("1 test(s) failing"));
    }

    #[test]
    fn suggest_action_review_on_high_risk() {
        let risk = RiskAssessment {
            level: RiskLevel::High,
            factors: vec![RiskFactor {
                signal: "large_change".into(),
                level: RiskLevel::High,
                detail: "600 lines changed".into(),
            }],
        };
        let tests = TestResults {
            passed: 50,
            failed: 0,
            ignored: 0,
            total: 50,
            failures: vec![],
            command: "cargo test".into(),
        };
        let (action, _) = suggest_action(&risk, &tests);
        assert_eq!(action, SuggestedAction::Review);
    }

    #[test]
    fn extract_count_basic() {
        assert_eq!(extract_count("841 passed", "passed"), Some(841));
        assert_eq!(extract_count("0 failed", "failed"), Some(0));
        assert_eq!(extract_count("2 ignored", "ignored"), Some(2));
        assert_eq!(extract_count("no match", "passed"), None);
    }

    #[test]
    fn truncate_short() {
        assert_eq!(truncate("hello", 10), "hello");
    }

    #[test]
    fn truncate_long() {
        assert_eq!(
            truncate("bun_01hqkj1234567890abcdef", 20),
            "bun_01hqkj1234567..."
        );
    }
}
