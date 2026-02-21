use crate::plan::schema::{CheckSpec, Plan};
use anyhow::{bail, Context, Result};
use std::path::Path;

/// Load and validate a plan from a YAML file.
pub fn load_plan(path: &Path) -> Result<Plan> {
    let content =
        std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    parse_plan(&content)
}

/// Parse and validate a plan from a YAML string.
pub fn parse_plan(yaml: &str) -> Result<Plan> {
    // Step 1: Parse into raw Value for short-format normalization
    let mut raw: serde_yml::Value = serde_yml::from_str(yaml).context("invalid YAML syntax")?;

    // Step 2: Normalize short-format checks
    normalize_checks(&mut raw)?;

    // Step 3: Expand variables
    expand_variables(&mut raw);

    // Step 4: Deserialize into typed Plan
    let plan: Plan = serde_yml::from_value(raw).context("plan schema validation failed")?;

    // Step 5: Validate constraints
    validate_plan(&plan)?;

    Ok(plan)
}

/// Normalize short-format checks into tagged objects.
///
/// Short: `cmd_succeeds: "cargo test"`
/// Long:  `{ type: cmd_succeeds, cmd: "cargo test" }`
fn normalize_checks(raw: &mut serde_yml::Value) -> Result<()> {
    let phases = match raw.get_mut("phases") {
        Some(serde_yml::Value::Sequence(seq)) => seq,
        _ => return Ok(()),
    };

    for phase in phases.iter_mut() {
        let checks = match phase.get_mut("check") {
            Some(serde_yml::Value::Sequence(seq)) => seq,
            _ => continue,
        };

        for check in checks.iter_mut() {
            if let Some(normalized) = normalize_one_check(check)? {
                *check = normalized;
            }
        }
    }
    Ok(())
}

/// Try to normalize a single short-format check.
/// Returns Some(normalized) if it was short format, None if already tagged.
fn normalize_one_check(check: &serde_yml::Value) -> Result<Option<serde_yml::Value>> {
    let map = match check.as_mapping() {
        Some(m) => m,
        None => bail!("check must be a mapping, got: {check:?}"),
    };

    // Already tagged format (has "type" key)
    if map.contains_key(serde_yml::Value::String("type".into())) {
        return Ok(None);
    }

    // Short format: single key = check type, value = argument
    if map.len() != 1 {
        bail!(
            "short-format check must have exactly one key, got {}",
            map.len()
        );
    }

    let (key, value) = map.iter().next().unwrap();
    let key_str = key
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("check key must be a string"))?;

    let mut out = serde_yml::Mapping::new();
    out.insert(
        serde_yml::Value::String("type".into()),
        serde_yml::Value::String(key_str.into()),
    );

    match key_str {
        "cmd_succeeds" => {
            let cmd = value
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("cmd_succeeds value must be a string"))?;
            out.insert(
                serde_yml::Value::String("cmd".into()),
                serde_yml::Value::String(cmd.into()),
            );
        }
        "file_exists" => {
            let path = value
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("file_exists value must be a string"))?;
            out.insert(
                serde_yml::Value::String("path".into()),
                serde_yml::Value::String(path.into()),
            );
        }
        "file_contains" => {
            // file_contains is a map: { path: ..., pattern: ... }
            let inner = value
                .as_mapping()
                .ok_or_else(|| anyhow::anyhow!("file_contains value must be a mapping"))?;
            for (k, v) in inner {
                out.insert(k.clone(), v.clone());
            }
        }
        "git_clean" => {
            // git_clean: true  or  git_clean: { allow_untracked: true }
            if let Some(m) = value.as_mapping() {
                for (k, v) in m {
                    out.insert(k.clone(), v.clone());
                }
            }
            // git_clean: true → no extra fields needed
        }
        "edda_event" => {
            if let Some(m) = value.as_mapping() {
                for (k, v) in m {
                    out.insert(k.clone(), v.clone());
                }
            } else {
                let et = value
                    .as_str()
                    .ok_or_else(|| anyhow::anyhow!("edda_event value must be string or mapping"))?;
                out.insert(
                    serde_yml::Value::String("event_type".into()),
                    serde_yml::Value::String(et.into()),
                );
            }
        }
        other => {
            bail!(
                "unknown check type: \"{other}\". Valid types: cmd_succeeds, file_exists, \
                 file_contains, git_clean, edda_event, wait_until"
            );
        }
    }

    Ok(Some(serde_yml::Value::Mapping(out)))
}

/// Expand `${{ env.VAR_NAME }}` patterns in string values.
fn expand_variables(value: &mut serde_yml::Value) {
    match value {
        serde_yml::Value::String(s) => {
            if s.contains("${{") {
                *s = expand_env_vars(s);
            }
        }
        serde_yml::Value::Mapping(m) => {
            for (_, v) in m.iter_mut() {
                expand_variables(v);
            }
        }
        serde_yml::Value::Sequence(seq) => {
            for v in seq.iter_mut() {
                expand_variables(v);
            }
        }
        _ => {}
    }
}

/// Replace `${{ env.VAR_NAME }}` with the environment variable value.
fn expand_env_vars(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let mut rest = s;

    while let Some(start) = rest.find("${{") {
        result.push_str(&rest[..start]);
        let after_start = &rest[start + 3..];
        if let Some(end) = after_start.find("}}") {
            let expr = after_start[..end].trim();
            if let Some(var_name) = expr.strip_prefix("env.") {
                let val = std::env::var(var_name.trim()).unwrap_or_default();
                result.push_str(&val);
            } else {
                // Unknown expression — keep as-is
                result.push_str(&rest[start..start + 3 + end + 2]);
            }
            rest = &after_start[end + 2..];
        } else {
            // Unclosed — keep as-is
            result.push_str(&rest[start..]);
            rest = "";
        }
    }
    result.push_str(rest);
    result
}

/// Validate plan constraints that can't be expressed in serde.
fn validate_plan(plan: &Plan) -> Result<()> {
    // Rule 1: name must be kebab-case
    if !is_kebab_case(&plan.name) {
        bail!(
            "plan name must be kebab-case (lowercase letters, digits, hyphens), got: \"{}\"",
            plan.name
        );
    }

    // Rule 2: at least one phase
    if plan.phases.is_empty() {
        bail!("plan must have at least one phase");
    }

    // Rule 3: phase IDs must be unique
    let mut seen_ids = std::collections::HashSet::new();
    for phase in &plan.phases {
        if !seen_ids.insert(phase.id.as_str()) {
            bail!("duplicate phase id: \"{}\"", phase.id);
        }
    }

    // Rule 4: depends_on must reference existing phase IDs
    for phase in &plan.phases {
        for dep in &phase.depends_on {
            if !seen_ids.contains(dep.as_str()) {
                bail!(
                    "phase \"{}\" depends on \"{dep}\" which does not exist",
                    phase.id
                );
            }
        }
    }

    // Rule 5: no cycles (delegated to topo module, checked separately)
    // Rule 6: wait_until cannot nest another wait_until
    for phase in &plan.phases {
        for check in &phase.check {
            validate_check_nesting(check)?;
        }
    }

    Ok(())
}

fn validate_check_nesting(check: &CheckSpec) -> Result<()> {
    if let CheckSpec::WaitUntil { check: inner, .. } = check {
        if matches!(inner.as_ref(), CheckSpec::WaitUntil { .. }) {
            bail!("wait_until cannot nest another wait_until");
        }
    }
    Ok(())
}

fn is_kebab_case(s: &str) -> bool {
    if s.is_empty() {
        return false;
    }
    // Must start with lowercase letter or digit
    let first = s.as_bytes()[0];
    if !(first.is_ascii_lowercase() || first.is_ascii_digit()) {
        return false;
    }
    s.bytes()
        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plan::schema::OnFail;

    #[test]
    fn parse_minimal_plan() {
        let yaml = r#"
name: test
phases:
  - id: one
    prompt: "hello"
"#;
        let plan = parse_plan(yaml).unwrap();
        assert_eq!(plan.name, "test");
        assert_eq!(plan.phases.len(), 1);
    }

    #[test]
    fn short_format_cmd_succeeds() {
        let yaml = r#"
name: test
phases:
  - id: build
    prompt: "build"
    check:
      - cmd_succeeds: "cargo build"
"#;
        let plan = parse_plan(yaml).unwrap();
        assert!(matches!(
            &plan.phases[0].check[0],
            CheckSpec::CmdSucceeds { cmd, .. } if cmd == "cargo build"
        ));
    }

    #[test]
    fn short_format_file_exists() {
        let yaml = r#"
name: test
phases:
  - id: one
    prompt: "x"
    check:
      - file_exists: "src/main.rs"
"#;
        let plan = parse_plan(yaml).unwrap();
        assert!(matches!(
            &plan.phases[0].check[0],
            CheckSpec::FileExists { path } if path == "src/main.rs"
        ));
    }

    #[test]
    fn short_format_file_contains() {
        let yaml = r#"
name: test
phases:
  - id: one
    prompt: "x"
    check:
      - file_contains:
          path: "Cargo.toml"
          pattern: "edda-core"
"#;
        let plan = parse_plan(yaml).unwrap();
        assert!(matches!(
            &plan.phases[0].check[0],
            CheckSpec::FileContains { path, pattern }
                if path == "Cargo.toml" && pattern == "edda-core"
        ));
    }

    #[test]
    fn short_format_git_clean() {
        let yaml = r#"
name: test
phases:
  - id: one
    prompt: "x"
    check:
      - git_clean: true
"#;
        let plan = parse_plan(yaml).unwrap();
        assert!(matches!(
            &plan.phases[0].check[0],
            CheckSpec::GitClean {
                allow_untracked: false
            }
        ));
    }

    #[test]
    fn tagged_format_passes_through() {
        let yaml = r#"
name: test
phases:
  - id: one
    prompt: "x"
    check:
      - type: cmd_succeeds
        cmd: "cargo test"
        timeout_sec: 300
"#;
        let plan = parse_plan(yaml).unwrap();
        assert!(matches!(
            &plan.phases[0].check[0],
            CheckSpec::CmdSucceeds { cmd, timeout_sec: 300 } if cmd == "cargo test"
        ));
    }

    #[test]
    fn variable_expansion() {
        std::env::set_var("EDDA_TEST_VAR_12345", "hello-world");
        let yaml = r#"
name: test
phases:
  - id: one
    prompt: "x"
    env:
      MY_VAR: "${{ env.EDDA_TEST_VAR_12345 }}"
"#;
        let plan = parse_plan(yaml).unwrap();
        assert_eq!(plan.phases[0].env.get("MY_VAR").unwrap(), "hello-world");
        std::env::remove_var("EDDA_TEST_VAR_12345");
    }

    #[test]
    fn reject_non_kebab_name() {
        let yaml = r#"
name: MyPlan
phases:
  - id: one
    prompt: "x"
"#;
        let err = parse_plan(yaml).unwrap_err();
        assert!(err.to_string().contains("kebab-case"));
    }

    #[test]
    fn reject_empty_phases() {
        let yaml = r#"
name: test
phases: []
"#;
        let err = parse_plan(yaml).unwrap_err();
        assert!(err.to_string().contains("at least one phase"));
    }

    #[test]
    fn reject_duplicate_phase_id() {
        let yaml = r#"
name: test
phases:
  - id: same
    prompt: "a"
  - id: same
    prompt: "b"
"#;
        let err = parse_plan(yaml).unwrap_err();
        assert!(err.to_string().contains("duplicate"));
    }

    #[test]
    fn reject_unknown_dependency() {
        let yaml = r#"
name: test
phases:
  - id: one
    prompt: "x"
    depends_on: [nonexistent]
"#;
        let err = parse_plan(yaml).unwrap_err();
        assert!(err.to_string().contains("nonexistent"));
    }

    #[test]
    fn reject_unknown_check_type() {
        let yaml = r#"
name: test
phases:
  - id: one
    prompt: "x"
    check:
      - magic_check: "foo"
"#;
        let err = parse_plan(yaml).unwrap_err();
        assert!(err.to_string().contains("unknown check type"));
    }

    #[test]
    fn reject_nested_wait_until() {
        let yaml = r#"
name: test
phases:
  - id: one
    prompt: "x"
    check:
      - type: wait_until
        interval_sec: 5
        timeout_sec: 30
        check:
          type: wait_until
          interval_sec: 1
          timeout_sec: 10
          check:
            type: file_exists
            path: "x"
"#;
        let err = parse_plan(yaml).unwrap_err();
        assert!(err.to_string().contains("wait_until cannot nest"));
    }

    #[test]
    fn on_fail_variants_deserialize() {
        for (input, expected) in [
            ("auto_retry", OnFail::AutoRetry),
            ("ask", OnFail::Ask),
            ("skip", OnFail::Skip),
            ("abort", OnFail::Abort),
        ] {
            let yaml = format!("name: test\non_fail: {input}\nphases:\n  - id: a\n    prompt: x\n");
            let plan = parse_plan(&yaml).unwrap();
            assert_eq!(plan.on_fail, expected, "failed for {input}");
        }
    }
}
