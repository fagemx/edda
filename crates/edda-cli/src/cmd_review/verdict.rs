use anyhow::{bail, Context, Result};
use edda_core::{ReviewChecklistItem, ReviewFinding, ReviewVerdictPayload};
use serde::Deserialize;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct EngineVerdict {
    pub subject_seen: String,
    pub verdict: String,
    findings: Vec<EngineFinding>,
    checklist: Vec<EngineChecklist>,
    pub escalations: Vec<String>,
    pub model_self_report: String,
    pub notes: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct EngineChecklist {
    item: String,
    result: String,
    measure: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct EngineFinding {
    severity: String,
    file: String,
    line: Option<u64>,
    claim: String,
    evidence: String,
    rule: String,
}

pub(crate) fn parse(text: &str, head: &str, measures: &[String]) -> Result<EngineVerdict> {
    let marker = "```edda-review-verdict/v1";
    if text.matches(marker).count() != 1 {
        bail!("expected exactly one edda-review-verdict/v1 block");
    }
    let tail = text.split_once(marker).context("missing verdict block")?.1;
    let (json, after) = tail
        .split_once("```")
        .context("unterminated verdict block")?;
    if !after.trim().is_empty() {
        bail!("verdict block must end the response");
    }
    let verdict: EngineVerdict = serde_json::from_str(json.trim())?;
    if verdict.subject_seen != head {
        bail!("subject-mismatch: marker does not match reviewed HEAD");
    }
    if !matches!(verdict.verdict.as_str(), "lgtm" | "changes-requested") {
        bail!("invalid verdict");
    }
    if verdict.checklist.is_empty() {
        bail!("missing scoped checklist");
    }
    for item in &verdict.checklist {
        if !matches!(item.result.as_str(), "ran" | "escalate" | "na")
            || item.item.trim().is_empty()
            || item.measure.trim().is_empty()
        {
            bail!("invalid checklist entry");
        }
        if item.result == "ran" && !measures.iter().any(|measure| measure == &item.measure) {
            bail!("checklist ran measure is not host-provided evidence");
        }
    }
    for finding in &verdict.findings {
        if !matches!(finding.severity.as_str(), "P0" | "P1" | "P2")
            || finding.file.trim().is_empty()
            || finding.claim.trim().is_empty()
            || finding.evidence.trim().is_empty()
            || finding.rule.trim().is_empty()
        {
            bail!("finding lacks valid severity or required evidence");
        }
    }
    if verdict.verdict == "lgtm"
        && verdict
            .findings
            .iter()
            .any(|f| matches!(f.severity.as_str(), "P0" | "P1"))
    {
        bail!("lgtm contradicts blocking findings");
    }
    Ok(verdict)
}

impl EngineVerdict {
    pub(crate) fn findings(&self) -> Vec<ReviewFinding> {
        self.findings
            .iter()
            .enumerate()
            .map(|(i, f)| ReviewFinding {
                id: format!("f{}", i + 1),
                severity: f.severity.clone(),
                file: f.file.clone(),
                line: f.line,
                claim: f.claim.clone(),
                evidence: f.evidence.clone(),
                rule: f.rule.clone(),
                status: "open".into(),
            })
            .collect()
    }

    pub(crate) fn checklist(&self) -> Vec<ReviewChecklistItem> {
        self.checklist
            .iter()
            .map(|item| ReviewChecklistItem {
                item: item.item.clone(),
                result: item.result.clone(),
                measure: item.measure.clone(),
            })
            .collect()
    }
}

pub(crate) fn qualify(payload: &mut ReviewVerdictPayload) {
    let mut reasons = Vec::new();
    for (bad, reason) in [
        (payload.verdict == "unreviewed", "unreviewed"),
        (payload.spec.mode != "spec-backed", "spec-convention-only"),
        (payload.gates.status == "undeclared", "gates-undeclared"),
        (payload.gates.status == "unverified", "gates-unverified"),
        (payload.gates.status == "red", "gates-red"),
        (
            payload.reviewer.model_observed == "unknown",
            "model-unknown",
        ),
        (payload.parse != "ok", "parse-failed"),
        (payload.subject.coverage != "full", "coverage-partial"),
        (payload.reviewer.tool_policy != "hard", "tool-policy-none"),
        (!payload.escalations.is_empty(), "escalation-pending"),
    ] {
        if bad {
            reasons.push(reason.to_owned());
        }
    }
    use edda_core::model_id::canonical_model_id;
    let requested = canonical_model_id(&payload.reviewer.model_requested);
    let observed = canonical_model_id(&payload.reviewer.model_observed);
    if requested.is_some() && observed.is_some() && requested != observed {
        reasons.push("model-mismatch".into());
    }
    if payload.independence_policy == "model" && payload.independence != "verified" {
        reasons.push(format!("independence-{}", payload.independence));
    }
    for reason in &payload.disqualifiers {
        if !reasons.contains(reason) {
            reasons.push(reason.clone());
        }
    }
    payload.qualified = reasons.is_empty();
    payload.disqualifiers = reasons;
}

pub(crate) fn exit_code(payload: &ReviewVerdictPayload) -> i32 {
    match payload.verdict.as_str() {
        "changes-requested" => 1,
        "lgtm" if payload.qualified => 0,
        "lgtm" => 3,
        _ => 2,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn valid() -> serde_json::Value {
        serde_json::json!({"subject_seen":"head", "verdict":"lgtm", "findings":[],"checklist":[{"item":"scope","result":"na","measure":"no declared commands"}],"escalations":[],"model_self_report":"self-report", "notes":""})
    }
    fn fenced(v: serde_json::Value) -> String {
        format!("```edda-review-verdict/v1\n{v}\n```")
    }
    #[test]
    fn malformed_or_contradictory_output_never_approves() {
        assert!(parse(&fenced(valid()), "head", &[]).is_ok());
        assert!(parse(&fenced(valid()), "other", &[]).is_err());
        let mut v = valid();
        v["ran"] = serde_json::json!(["cargo test"]);
        assert!(parse(&fenced(v), "head", &[]).is_err());
        let mut v = valid();
        v["findings"] = serde_json::json!([{"severity":"P1","file":"x","line":1,"claim":"bad","evidence":"x:1","rule":"core"}]);
        assert!(parse(&fenced(v), "head", &[]).is_err());
        let mut v = valid();
        v["checklist"] = serde_json::json!([]);
        assert!(parse(&fenced(v), "head", &[]).is_err());
    }

    #[test]
    fn checklist_ran_must_cite_exact_host_evidence() {
        let mut v = valid();
        v["checklist"] = serde_json::json!([{
            "item":"gate",
            "result":"ran",
            "measure":"EVIDENCE RAN: cargo test -p edda"
        }]);
        assert!(parse(&fenced(v.clone()), "head", &[]).is_err());
        assert!(parse(
            &fenced(v),
            "head",
            &["EVIDENCE RAN: cargo test -p edda".into()]
        )
        .is_ok());
    }
}
