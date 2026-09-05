use super::process;
use anyhow::{Context, Result};
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};

/// gh pr checks legitimately exits 1 or 8 when its valid JSON describes red
/// or pending checks. Required names come from it; results come ONLY from SHA.
pub(crate) fn gh_required_checks(
    repo: &Path,
    pr: u64,
    head_sha: &str,
) -> Result<Vec<(String, String)>> {
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
        return Ok(vec![]);
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
    Ok(required_at_sha(&names, &checks, head_sha))
}

fn decode(output: &process::Output, allow_nonzero: bool) -> Result<Value> {
    anyhow::ensure!(!output.timed_out, "GitHub evidence deadline exceeded");
    anyhow::ensure!(
        !output.truncated,
        "GitHub evidence exceeds bounded JSON capture"
    );
    let value: Value = serde_json::from_slice(&output.stdout).context("GitHub evidence JSON")?;
    anyhow::ensure!(
        output.exit == 0 || (allow_nonzero && matches!(output.exit, 1 | 8) && value.is_array()),
        "GitHub evidence command exited {}",
        output.exit
    );
    Ok(value)
}

fn required_at_sha(names: &[String], runs: &[Value], sha: &str) -> Vec<(String, String)> {
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
    names
        .iter()
        .map(|name| {
            let bucket = match latest.get(name.as_str()) {
                Some(run) if run["status"] == "completed" => match run["conclusion"].as_str() {
                    Some("success" | "skipped") => "pass",
                    Some(
                        "failure" | "cancelled" | "timed_out" | "action_required"
                        | "startup_failure",
                    ) => "fail",
                    _ => "pending",
                },
                _ => "pending",
            };
            (name.clone(), bucket.into())
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
    fn missing_required_and_wrong_sha_cannot_disappear() {
        let names = vec!["A".into(), "B".into(), "C".into()];
        let runs = vec![
            json!({"name":"A","id":1,"head_sha":"head","status":"completed","conclusion":"success"}),
            json!({"name":"B","id":2,"head_sha":"other","status":"completed","conclusion":"success"}),
        ];
        assert_eq!(
            required_at_sha(&names, &runs, "head"),
            vec![
                ("A".into(), "pass".into()),
                ("B".into(), "pending".into()),
                ("C".into(), "pending".into())
            ]
        );
    }

    #[test]
    fn latest_attempt_and_neutral_are_not_green() {
        let runs = vec![
            json!({"name":"A","id":2,"head_sha":"head","status":"completed","conclusion":"neutral"}),
            json!({"name":"A","id":1,"head_sha":"head","status":"completed","conclusion":"success"}),
        ];
        assert_eq!(
            required_at_sha(&["A".into()], &runs, "head")[0].1,
            "pending"
        );
    }
}
