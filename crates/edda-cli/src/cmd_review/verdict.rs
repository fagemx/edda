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
    let mut verdict: EngineVerdict = serde_json::from_str(json.trim())?;
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
    // A scoped checklist escalation remains material even when the engine
    // omitted it from the redundant summary list. Preserve it so qualification
    // rejects an otherwise-LGTM verdict instead of losing the concern.
    for item in &verdict.checklist {
        if item.result == "escalate" && !verdict.escalations.contains(&item.item) {
            verdict.escalations.push(item.item.clone());
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
        (
            payload.verdict == "lgtm"
                && payload
                    .findings
                    .iter()
                    .any(|finding| matches!(finding.severity.as_str(), "P0" | "P1")),
            "blocking-findings-open",
        ),
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
    use edda_core::{
        ReviewBrief, ReviewCost, ReviewGates, ReviewRefs, ReviewReviewer, ReviewSpec, ReviewSubject,
    };
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

    #[test]
    fn escalated_checklist_item_is_preserved_when_summary_omits_it() {
        let mut v = valid();
        v["checklist"] = serde_json::json!([{
            "item":"missing exact-head CI receipt",
            "result":"escalate",
            "measure":"CI receipt unavailable"
        }]);
        let parsed = parse(&fenced(v.clone()), "head", &[]).unwrap();
        assert_eq!(parsed.escalations, ["missing exact-head CI receipt"]);
        v["escalations"] = serde_json::json!(["missing exact-head CI receipt"]);
        assert!(parse(&fenced(v), "head", &[]).is_ok());
    }

    fn qualified_payload_with_findings(findings: Vec<ReviewFinding>) -> ReviewVerdictPayload {
        ReviewVerdictPayload {
            schema: "review_verdict/0".into(),
            subject: ReviewSubject {
                base_sha: "base".into(),
                head_sha: "head".into(),
                files: 1,
                lines: 1,
                coverage: "full".into(),
                subject_seen: Some("head".into()),
                worktree_check: Some("unchanged".into()),
            },
            refs: ReviewRefs {
                pr: None,
                issue: None,
                supersedes: None,
                previous: Some("prior-event".into()),
                round: Some(2),
                history_rewritten: false,
            },
            spec: ReviewSpec {
                mode: "spec-backed".into(),
                source: "acceptance.txt".into(),
                trust: "local".into(),
            },
            brief: ReviewBrief {
                core: "review scope".into(),
                review_md_sha: None,
                classes: vec![],
            },
            reviewer: ReviewReviewer {
                agent: "pi".into(),
                transport: "rpc".into(),
                model_requested: "gpt-5.6-sol".into(),
                model_observed: "gpt-5.6-sol".into(),
                observed_via: "provider".into(),
                model_self_report: None,
                session_id: "session".into(),
                session_label: "review".into(),
                tool_policy: "hard".into(),
            },
            independence: "verified".into(),
            independence_policy: "session".into(),
            gates: ReviewGates {
                status: "verified".into(),
                declared_by: vec![],
                read: vec![],
                ran: vec![],
            },
            probes: vec![],
            verdict: "lgtm".into(),
            outcome: "done".into(),
            qualified: false,
            disqualifiers: vec![],
            findings,
            checklist: vec![],
            escalations: vec![],
            cost: ReviewCost {
                usd: None,
                measured: false,
                duration_ms: 0,
            },
            parse: "ok".into(),
            notes: None,
        }
    }

    #[test]
    fn final_blocking_findings_disqualify_lgtm_and_never_exit_zero() {
        let mut payload = qualified_payload_with_findings(vec![ReviewFinding {
            id: "f1".into(),
            severity: "P1".into(),
            file: "b.txt".into(),
            line: Some(1),
            claim: "prior same-head finding remains open".into(),
            evidence: "b.txt:1".into(),
            rule: "core".into(),
            status: "open".into(),
        }]);
        qualify(&mut payload);
        assert!(!payload.qualified);
        assert!(payload
            .disqualifiers
            .contains(&"blocking-findings-open".into()));
        assert_ne!(exit_code(&payload), 0);
    }
}
