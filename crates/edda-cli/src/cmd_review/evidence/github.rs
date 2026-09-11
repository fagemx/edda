use super::process;
use anyhow::{Context, Result};
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};

#[derive(Debug, Default)]
pub(crate) struct GitHubChecks {
    pub required_names: Vec<String>,
    pub latest_runs: BTreeMap<String, String>,
}

/// gh pr checks legitimately exits 1 or 8 when its valid JSON describes red
/// or pending checks. Required names come from it; results come ONLY from SHA.
pub(crate) fn gh_required_checks(repo: &Path, pr: u64, head_sha: &str) -> Result<GitHubChecks> {
    anyhow::ensure!(
        head_sha.len() == 40 && head_sha.bytes().all(|b| b.is_ascii_hexdigit()),
        "CI requires a full head SHA"
    );
    let deadline = Instant::now() + Duration::from_secs(60);
    let gh = process::executable("gh")?;
    let query = |args: &[&str], allow_nonzero: bool| -> Result<Value> {
        let output = process::run(
            Command::new(&gh).args(args),
            repo,
            deadline,
            4 * 1024 * 1024,
        )?;
        decode(&output, allow_nonzero)
    };
    let required = query(
        &[
            "pr",
            "checks",
            &pr.to_string(),
            "--required",
            "--json",
            "name",
        ],
        true,
    )?;
    let names = required
        .as_array()
        .context("required checks must be an array")?
        .iter()
        .map(|r| {
            r["name"]
                .as_str()
                .map(str::to_owned)
                .context("required check missing name")
        })
        .collect::<Result<Vec<_>>>()?;
    if names.is_empty() {
        // No required set means no trustworthy boundary for optional jobs.
        // Do not fetch them and never let one opportunistic success turn green.
        return Ok(GitHubChecks::default());
    }
    let endpoint = format!("repos/{{owner}}/{{repo}}/commits/{head_sha}/check-runs?per_page=100");
    let pages = query(&["api", &endpoint, "--paginate", "--slurp"], false)?;
    let mut checks = Vec::new();
    for page in pages
        .as_array()
        .context("check-run pages must be an array")?
    {
        checks.extend(
            page["check_runs"]
                .as_array()
                .context("check-runs missing array")?
                .iter()
                .cloned(),
        );
    }
    Ok(GitHubChecks {
        required_names: names,
        latest_runs: latest_at_sha(&checks, head_sha),
    })
}

fn decode(output: &process::Output, allow_nonzero: bool) -> Result<Value> {
    anyhow::ensure!(!output.timed_out, "GitHub evidence deadline exceeded");
    anyhow::ensure!(
        !output.truncated,
        "GitHub evidence exceeds bounded JSON capture"
    );
    if output.stdout.is_empty() && allow_nonzero && matches!(output.exit, 1 | 8) {
        return Ok(Value::Array(vec![]));
    }
    let value: Value = serde_json::from_slice(&output.stdout).context("GitHub evidence JSON")?;
    anyhow::ensure!(
        output.exit == 0 || (allow_nonzero && matches!(output.exit, 1 | 8) && value.is_array()),
        "GitHub evidence command exited {}",
        output.exit
    );
    Ok(value)
}

fn latest_at_sha(runs: &[Value], sha: &str) -> BTreeMap<String, String> {
    let mut latest: BTreeMap<&str, &Value> = BTreeMap::new();
    for run in runs {
        if run["head_sha"].as_str() != Some(sha) {
            continue;
        }
        if let Some(name) = run["name"].as_str() {
            let replace = latest.get(name).is_none_or(|old| {
                run["id"].as_u64().unwrap_or(0) > old["id"].as_u64().unwrap_or(0)
            });
            if replace {
                latest.insert(name, run);
            }
        }
    }
    latest
        .into_iter()
        .map(|(name, run)| {
            let bucket = match run["status"].as_str() {
                Some("completed") => match run["conclusion"].as_str() {
                    Some("success") => "pass",
                    Some("skipped") => "skipped",
                    Some(
                        "failure" | "cancelled" | "timed_out" | "action_required"
                        | "startup_failure",
                    ) => "fail",
                    _ => "pending",
                },
                _ => "pending",
            };
            (name.to_owned(), bucket.to_owned())
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn gh_nonzero_valid_check_json_is_evidence_but_api_errors_are_not() {
        for exit in [0, 1, 8] {
            let output = process::Output {
                exit,
                timed_out: false,
                stdout: br#"[{"name":"CI"}]"#.to_vec(),
                truncated: false,
            };
            assert!(decode(&output, true).is_ok());
            assert_eq!(decode(&output, false).is_ok(), exit == 0);
        }
        let output = process::Output {
            exit: 1,
            timed_out: false,
            stdout: br#"{"message":"forbidden"}"#.to_vec(),
            truncated: false,
        };
        assert!(decode(&output, true).is_err());
        let output = process::Output {
            exit: 0,
            timed_out: false,
            stdout: b"[]".to_vec(),
            truncated: true,
        };
        assert!(decode(&output, true).is_err());
    }

    #[test]
    fn empty_required_checks_are_tolerated_only_for_known_nonzero_exits() {
        for exit in [1, 8] {
            let output = process::Output {
                exit,
                timed_out: false,
                stdout: vec![],
                truncated: false,
            };
            assert_eq!(decode(&output, true).unwrap(), json!([]));
            assert!(decode(&output, false).is_err());
        }
        for exit in [0, 2] {
            let output = process::Output {
                exit,
                timed_out: false,
                stdout: vec![],
                truncated: false,
            };
            assert!(decode(&output, true).is_err());
        }
        let malformed = process::Output {
            exit: 1,
            timed_out: false,
            stdout: b"not JSON".to_vec(),
            truncated: false,
        };
        assert!(decode(&malformed, true).is_err());
    }

    #[test]
    fn latest_attempt_wins_and_wrong_sha_is_excluded() {
        let runs = vec![
            json!({"name":"A","id":1,"head_sha":"head","status":"completed","conclusion":"success"}),
            json!({"name":"A","id":99,"head_sha":"other","status":"completed","conclusion":"success"}),
            json!({"name":"A","id":2,"head_sha":"head","status":"completed","conclusion":"failure"}),
            json!({"name":"OnlyOther","id":3,"head_sha":"other","status":"completed","conclusion":"success"}),
        ];
        let latest = latest_at_sha(&runs, "head");
        assert_eq!(latest.get("A").map(String::as_str), Some("fail"));
        assert!(!latest.contains_key("OnlyOther"));
    }

    #[test]
    fn skipped_is_separate_and_neutral_is_pending() {
        let runs = vec![
            json!({"name":"A","id":2,"head_sha":"head","status":"completed","conclusion":"neutral"}),
            json!({"name":"A","id":1,"head_sha":"head","status":"completed","conclusion":"success"}),
            json!({"name":"B","id":1,"head_sha":"head","status":"completed","conclusion":"skipped"}),
        ];
        let latest = latest_at_sha(&runs, "head");
        assert_eq!(latest.get("A").map(String::as_str), Some("pending"));
        assert_eq!(latest.get("B").map(String::as_str), Some("skipped"));
    }
}
