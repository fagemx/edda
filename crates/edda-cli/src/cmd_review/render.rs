use edda_core::ReviewVerdictPayload;
pub(crate) fn render(p: &ReviewVerdictPayload, event: &str) -> String {
    let mut out = format!("review_verdict {event} · round {:?}\nhead {} · base {}\nverdict: {} · qualified: {}\nreviewer: {} · requested {} · observed {} ({})\nsession: {} · independence {} (policy {}) · tools {}\ngates: {}\nfindings: {}\n", p.refs.round, p.subject.head_sha, p.subject.base_sha, p.verdict, p.qualified, p.reviewer.agent, p.reviewer.model_requested, p.reviewer.model_observed, p.reviewer.observed_via, p.reviewer.session_id, p.independence, p.independence_policy, p.reviewer.tool_policy, p.gates.status, p.findings.len());
    for f in &p.findings {
        out.push_str(&format!(
            "{} {}:{} — {} [{}]\n",
            f.severity,
            f.file,
            f.line.unwrap_or(0),
            f.claim,
            f.evidence
        ));
    }
    for reason in &p.disqualifiers {
        let action = match reason.as_str() {
            "spec-convention-only" => "pass --spec <path|#issue>",
            "gates-undeclared" => "declare gates in REVIEW.md or pass --gate",
            "gates-unverified" => {
                "record edda run -- <gate> at this SHA on a clean tree, or pass --run-gates"
            }
            "gates-red" => "fix the failing gate and review the new SHA",
            "model-unknown" | "tool-policy-none" => {
                "choose pi or claude with observed model and enforced read-only tools"
            }
            "model-mismatch" => "inspect requested/observed model and select the intended model",
            "coverage-partial" => "review a smaller range or raise EDDA_REVIEW_DIFF_BUDGET_CHARS",
            "escalation-pending" => "resolve the listed review escalations",
            "session-mismatch" | "session-unverified" => {
                "inspect the backend session before resuming"
            }
            "independence-same-model" | "independence-unverified" => {
                "choose a verifiably different reviewer model or use session policy"
            }
            _ => "resolve the reported review failure and retry",
        };
        out.push_str(&format!("{reason} → {action}\n"));
    }
    let cost = p
        .cost
        .usd
        .map(|v| format!("${v:.4} (measured)"))
        .unwrap_or_else(|| "unmeasured (n/a)".into());
    out.push_str(&format!(
        "cost: {cost} · elapsed {} ms\n",
        p.cost.duration_ms
    ));
    if let Some(previous) = p.refs.supersedes.as_ref().or(p.refs.previous.as_ref()) {
        out.push_str(&format!(
            "previous: {previous} · history rewritten: {}\n",
            p.refs.history_rewritten
        ));
    }
    if let Some(notes) = &p.notes {
        out.push_str(notes);
        out.push('\n');
    }
    out
}
