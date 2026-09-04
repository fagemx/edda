//! GH-603 design-schema preview. This is deliberately not an executable Plan.
//! Runtime parsing rejects these fields until carrier runners ship.
use super::{parser::parse_plan, schema::Plan};
use anyhow::{bail, Context, Result};
use serde::Deserialize;
use serde_yml::Value;
use std::path::Path;

#[derive(Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum Basis {
    Git { sha: String },
    Document { uri: String, version: String },
}

impl Basis {
    fn validate(&self) -> Result<()> {
        match self {
            Self::Git { sha } => full_sha(sha),
            Self::Document { uri, version } => {
                nonempty(uri, "document uri")?;
                nonempty(version, "document immutable version")
            }
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum Deliverable {
    Pr {
        repository: String,
        number: u64,
        head_sha: String,
    },
    Finding {
        finding_id: String,
        basis: Basis,
    },
    Draft {
        path: String,
        version: String,
        decision_refs: Vec<String>,
    },
}

impl Deliverable {
    fn validate(&self) -> Result<()> {
        match self {
            Self::Pr {
                repository,
                number,
                head_sha,
            } => {
                nonempty(repository, "PR repository")?;
                if *number == 0 {
                    bail!("PR number must be positive");
                }
                full_sha(head_sha)
            }
            Self::Finding { finding_id, basis } => {
                nonempty(finding_id, "finding_id")?;
                basis.validate()
            }
            Self::Draft {
                path,
                version,
                decision_refs,
            } => {
                nonempty(path, "draft path")?;
                nonempty(version, "draft immutable version")?;
                if decision_refs.is_empty() {
                    bail!("draft requires decision_refs");
                }
                for decision in decision_refs {
                    nonempty(decision, "decision reference")?;
                }
                Ok(())
            }
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Isolation {
    None,
    Scratch,
    Worktree,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
enum PreviewCheck {
    Fresh {
        source: String,
        instance: String,
        within_sec: Option<u64>,
    },
    FindingVerdict {
        finding_id: String,
        basis: Basis,
    },
}

impl PreviewCheck {
    fn validate(&self) -> Result<()> {
        match self {
            Self::Fresh {
                source,
                instance,
                within_sec,
            } => {
                nonempty(source, "fresh source")?;
                nonempty(instance, "fresh instance")?;
                if *within_sec == Some(0) {
                    bail!("within_sec must be positive");
                }
                Ok(())
            }
            Self::FindingVerdict { finding_id, basis } => {
                nonempty(finding_id, "finding_id")?;
                basis.validate()
            }
        }
    }
}

fn nonempty(value: &str, label: &str) -> Result<()> {
    if value.trim().is_empty() {
        bail!("{label} must not be empty");
    }
    Ok(())
}

fn full_sha(sha: &str) -> Result<()> {
    if sha.len() != 40 || !sha.bytes().all(|b| b.is_ascii_hexdigit()) {
        bail!("basis/head_sha must be a full 40-character git SHA");
    }
    Ok(())
}

fn preview_check(check: &Value) -> bool {
    matches!(
        check.get("type").and_then(Value::as_str),
        Some("fresh" | "finding_verdict")
    )
}

/// Guard the normal parser, including callers other than the CLI.
pub(super) fn reject_runtime_extensions(raw: &Value) -> Result<()> {
    reject_strategy_override(raw)?;
    if raw.get("strategy_run_id").is_some() {
        bail!("strategy_run_id is schema-preview only; use conduct run --dry-run (GH-603)");
    }
    if let Some(phases) = raw.get("phases").and_then(Value::as_sequence) {
        for phase in phases {
            if ["deliverable", "isolation", "owns_objects"]
                .iter()
                .any(|key| phase.get(*key).is_some())
            {
                bail!(
                    "carrier/isolation schema is preview-only; use conduct run --dry-run (GH-603)"
                );
            }
            if let Some(checks) = phase.get("check").and_then(Value::as_sequence) {
                for check in checks {
                    reject_preview_check(check)?;
                }
            }
        }
    }
    Ok(())
}

fn reject_preview_check(check: &Value) -> Result<()> {
    if preview_check(check) {
        bail!("fresh/finding_verdict checks are schema-preview only; use conduct run --dry-run (GH-603)");
    }
    if let Some(inner) = check.get("check") {
        reject_preview_check(inner)?;
    }
    Ok(())
}

/// Typed validation of draft fields, then existing plan validation/topology.
/// The projected Plan omits draft checks and MUST only be used for display.
pub fn load_preview(path: &Path) -> Result<Plan> {
    let yaml =
        std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    parse_preview(&yaml)
}

fn parse_preview(yaml: &str) -> Result<Plan> {
    let mut raw: Value = serde_yml::from_str(yaml).context("invalid YAML syntax")?;
    reject_strategy_override(&raw)?;
    if let Some(map) = raw.as_mapping_mut() {
        if let Some(id) = map.remove("strategy_run_id") {
            let id: String =
                serde_yml::from_value(id).context("strategy_run_id must be a string")?;
            nonempty(&id, "strategy_run_id")?;
        }
    }
    if let Some(phases) = raw.get_mut("phases").and_then(Value::as_sequence_mut) {
        for phase in phases {
            if let Some(map) = phase.as_mapping_mut() {
                if let Some(value) = map.remove("deliverable") {
                    serde_yml::from_value::<Deliverable>(value)
                        .context("deliverable schema")?
                        .validate()?;
                }
                if let Some(value) = map.remove("isolation") {
                    serde_yml::from_value::<Isolation>(value).context("isolation schema")?;
                }
                if let Some(value) = map.remove("owns_objects") {
                    let objects: Vec<String> =
                        serde_yml::from_value(value).context("owns_objects schema")?;
                    for object in objects {
                        let Some((kind, id)) = object.split_once(':') else {
                            bail!("object claim requires kind:id");
                        };
                        if !matches!(kind, "finding" | "source" | "draft") {
                            bail!("unknown object claim kind: {kind}");
                        }
                        nonempty(id, "object claim id")?;
                    }
                }
            }
            if let Some(checks) = phase.get_mut("check").and_then(Value::as_sequence_mut) {
                let mut legacy = Vec::new();
                for check in checks.drain(..) {
                    if !validate_preview_check(&check)? {
                        legacy.push(check);
                    }
                }
                *checks = legacy;
            }
        }
    }
    let plan = parse_plan(&serde_yml::to_string(&raw)?)?;
    super::topo::topo_sort(&plan)?;
    Ok(plan)
}

fn reject_strategy_override(raw: &Value) -> Result<()> {
    if raw.get("strategy").is_some() {
        bail!("strategy belongs to the wave adapter, not a conductor plan");
    }
    if let Some(phases) = raw.get("phases").and_then(Value::as_sequence) {
        if phases
            .iter()
            .any(|phase| phase.get("strategy_run_id").is_some())
        {
            bail!("strategy_run_id belongs to the plan; phases cannot override it");
        }
    }
    Ok(())
}

// Return true only when the entire check is a validated draft check.
fn validate_preview_check(check: &Value) -> Result<bool> {
    if preview_check(check) {
        serde_yml::from_value::<PreviewCheck>(check.clone())
            .context("preview check schema")?
            .validate()?;
        return Ok(true);
    }
    // Nested draft checks are intentionally outside this first schema draft.
    if let Some(inner) = check.get("check") {
        reject_preview_check(inner)?;
    }
    Ok(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn examples_validate_but_cannot_execute() {
        for yaml in [
            include_str!("../../../../docs/design/infra-contracts/coding.yaml"),
            include_str!("../../../../docs/design/infra-contracts/research.yaml"),
            include_str!("../../../../docs/design/infra-contracts/loop.yaml"),
        ] {
            parse_preview(yaml).unwrap();
            assert!(parse_plan(yaml)
                .unwrap_err()
                .to_string()
                .contains("preview"));
        }
    }

    #[test]
    fn malformed_drafts_and_unknown_checks_fail() {
        let yaml = include_str!("../../../../docs/design/infra-contracts/research.yaml");
        for bad in [
            yaml.replace("580e98678fe6a39f57ad7a4dcbff74ecf47f2be4", "580e986"),
            yaml.replace("isolation: scratch", "isolation: magic"),
            yaml.replace("kind: finding", "kind: unknown"),
            yaml.replace("type: finding_verdict", "type: unknown"),
            yaml.replace("finding_id: finding-587", "finding_id: ''"),
            yaml.replace("strategy_run_id: review-587", "strategy_run_id: ''"),
        ] {
            assert!(parse_preview(&bad).is_err(), "accepted {bad}");
        }
        let loop_yaml = include_str!("../../../../docs/design/infra-contracts/loop.yaml");
        assert!(parse_preview(&loop_yaml.replace("within_sec: 60", "within_sec: 0")).is_err());
    }

    #[test]
    fn legacy_plans_and_legacy_errors_are_preserved() {
        let yaml = "name: legacy\nphases:\n  - id: one\n    prompt: hi\n    check:\n      - file_exists: x\n";
        assert_eq!(parse_preview(yaml).unwrap().phases[0].check.len(), 1);
        assert!(parse_plan(yaml).is_ok());
        assert!(parse_preview(&yaml.replace("name: legacy", "name: Invalid")).is_err());
    }

    #[test]
    fn each_runtime_extension_is_rejected_even_without_a_run_stamp() {
        for fields in [
            "    deliverable: null\n",
            "    isolation: none\n",
            "    owns_objects: []\n",
            "    check:\n      - type: fresh\n        source: heartbeat\n        instance: lane\n",
            "    check:\n      - type: wait_until\n        check:\n          type: fresh\n          source: heartbeat\n          instance: lane\n",
        ] {
            let yaml = format!("name: guarded\nphases:\n  - id: one\n    prompt: hi\n{fields}");
            assert!(parse_plan(&yaml).unwrap_err().to_string().contains("preview"));
        }
    }

    #[test]
    fn draft_requires_version_and_decision_references() {
        let yaml = "name: draft\nphases:\n  - id: one\n    prompt: hi\n    deliverable:\n      kind: draft\n      path: draft.md\n      version: sha256:example\n      decision_refs: [decision-1]\n";
        parse_preview(yaml).unwrap();
        assert!(parse_preview(&yaml.replace("[decision-1]", "[]")).is_err());
        assert!(parse_preview(&yaml.replace("sha256:example", "''")).is_err());
        assert!(
            parse_preview(&yaml.replace("path: draft.md", "path: draft.md\n      typo: true"))
                .is_err()
        );
    }
}
