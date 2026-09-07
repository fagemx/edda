//! `--by-rule <RULE>` — rule-based ratification (GH-761).
//!
//! `decision.auto-ratify` says the machine ratifies by rule and the operator
//! reads exceptions. Before this module that rule was a regex sweep in a
//! controller's home directory: not in the product, not tested, invisible to
//! every other machine, and its audit trail was a free-text `--by`. Here the
//! rule is a pure function over decisions, so the same input yields the same
//! verdicts on every machine, and each verdict carries the reason it fired.
//!
//! The engine is deliberately ledger-free: `evaluate` takes candidates and
//! returns verdicts. Reading the ledger and writing events is the caller's
//! job (`super::run`), which keeps the rule itself exhaustively testable.

use std::collections::BTreeSet;

/// The only rule that ships today. An unknown name is a usage error, never a
/// silent no-op sweep.
pub const RULE_CITED_AUTHORITY: &str = "cited-authority";

/// Key prefixes the rule never ratifies: these bind money or product
/// promises, and an agent citing an issue is not authority for either.
const HELD_PREFIXES: [&str; 3] = ["product.", "commercial.", "spend."];

/// Citation kinds `--cite` accepts and the rule recognises.
pub const CITATION_KINDS: [&str; 3] = ["operator:", "issue:", "decision:"];

/// One active decision, as the rule sees it.
#[derive(Debug, Clone)]
pub struct Candidate {
    pub key: String,
    pub reason: String,
    /// Structured `--cite` values (GH-761). Empty for decisions written
    /// before the flag existed — those fall back to scanning `reason`.
    pub cites: Vec<String>,
    /// Already operator- or rule-ratified. Kept in the input (rather than
    /// filtered out by the caller) because a ratified decision can still be
    /// the *later* decision that supersedes an unratified one.
    pub ratified: bool,
    /// Ledger insertion order. Only ever compared, never displayed.
    pub order: i64,
}

/// What the rule decided, and why. The why is not decoration: it is the
/// column the operator reads to see what the machine did on their behalf.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Ratify { citation: String },
    Hold { why: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Verdict {
    pub key: String,
    pub outcome: Outcome,
}

impl Verdict {
    pub fn is_ratify(&self) -> bool {
        matches!(self.outcome, Outcome::Ratify { .. })
    }

    /// The `action` and `why` columns of the printed table.
    pub fn columns(&self) -> (&'static str, &str) {
        match &self.outcome {
            Outcome::Ratify { citation } => ("ratify", citation.as_str()),
            Outcome::Hold { why } => ("hold", why.as_str()),
        }
    }
}

/// True when `c` is a well-formed citation (`<kind>:<non-empty>`).
pub fn is_citation(c: &str) -> bool {
    CITATION_KINDS.iter().any(|k| {
        c.strip_prefix(k)
            .is_some_and(|rest| !rest.trim().is_empty())
    })
}

/// Reject a malformed `--cite` value at the boundary (exit 2), so a typo
/// never lands in the ledger as an uninterpretable citation.
pub fn validate_citation(c: &str) -> Result<(), String> {
    if is_citation(c) {
        return Ok(());
    }
    Err(format!(
        "citation must be one of {} — got '{c}'",
        CITATION_KINDS.join(" ")
    ))
}

/// Apply `cited-authority` to every candidate, returning one verdict per
/// **unratified** candidate in input order.
///
/// `binding_keys` are the keys already binding in this branch: naming one in
/// a reason is the "cites a binding decision" arm of the rule.
pub fn evaluate(all: &[Candidate], binding_keys: &BTreeSet<String>) -> Vec<Verdict> {
    all.iter()
        .filter(|c| !c.ratified)
        .map(|c| Verdict {
            key: c.key.clone(),
            outcome: verdict_for(c, all, binding_keys),
        })
        .collect()
}

fn verdict_for(c: &Candidate, all: &[Candidate], binding_keys: &BTreeSet<String>) -> Outcome {
    if let Some(prefix) = HELD_PREFIXES.iter().find(|p| c.key.starts_with(**p)) {
        return Outcome::Hold {
            why: format!("held domain '{prefix}' — operator ratifies these"),
        };
    }
    if let Some(later) = superseding(c, all) {
        return Outcome::Hold {
            why: format!("superseded — a later decision '{later}' names it"),
        };
    }
    match citation(c, binding_keys) {
        Some(citation) => Outcome::Ratify { citation },
        None => Outcome::Hold {
            why: "no citation — add --cite operator:<when> | issue:#<n> | decision:<key>"
                .to_string(),
        },
    }
}

/// The key of a later decision whose reason names `c`, if any. This is the
/// soft supersede the ledger's own `supersedes` edge does not catch: a new
/// decision that explains itself by pointing at an older one has already
/// overtaken it, whatever its key.
fn superseding(c: &Candidate, all: &[Candidate]) -> Option<String> {
    all.iter()
        .filter(|o| o.order > c.order && o.key != c.key)
        .find(|o| o.reason.contains(&c.key))
        .map(|o| o.key.clone())
}

/// The citation this decision rests on: the structured field when present,
/// otherwise whatever the prose reason can be read to cite.
fn citation(c: &Candidate, binding_keys: &BTreeSet<String>) -> Option<String> {
    if let Some(found) = c.cites.iter().find(|x| is_citation(x)) {
        return Some(found.trim().to_string());
    }
    citation_from_reason(&c.reason, &c.key, binding_keys)
}

/// Fallback for decisions written before `--cite` existed. Ordered most
/// machine-checkable first: an issue number, then an exact binding key, then
/// the word "operator".
fn citation_from_reason(
    reason: &str,
    own_key: &str,
    binding_keys: &BTreeSet<String>,
) -> Option<String> {
    if let Some(n) = issue_number(reason) {
        return Some(format!("issue:#{n} (from reason)"));
    }
    if let Some(k) = binding_keys
        .iter()
        .filter(|k| k.as_str() != own_key)
        .find(|k| reason.contains(k.as_str()))
    {
        return Some(format!("decision:{k} (from reason)"));
    }
    let lower = reason.to_lowercase();
    if lower.contains("operator") || reason.contains("操作者") {
        return Some("operator (from reason)".to_string());
    }
    None
}

/// First `#<digits>` or `GH-<digits>` in the text.
fn issue_number(text: &str) -> Option<String> {
    let chars: Vec<char> = text.chars().collect();
    for i in 0..chars.len() {
        let start = if chars[i] == '#' {
            Some(i + 1)
        } else if (chars[i] == 'G' || chars[i] == 'g')
            && chars.get(i + 1).is_some_and(|c| *c == 'H' || *c == 'h')
            && chars.get(i + 2) == Some(&'-')
        {
            Some(i + 3)
        } else {
            None
        };
        if let Some(s) = start {
            let digits: String = chars[s..]
                .iter()
                .take_while(|c| c.is_ascii_digit())
                .collect();
            if !digits.is_empty() {
                return Some(digits);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cand(key: &str, reason: &str, order: i64) -> Candidate {
        Candidate {
            key: key.to_string(),
            reason: reason.to_string(),
            cites: Vec::new(),
            ratified: false,
            order,
        }
    }

    fn none() -> BTreeSet<String> {
        BTreeSet::new()
    }

    #[test]
    fn cited_decision_is_ratified() {
        let mut c = cand("db.engine", "embedded, zero-config", 1);
        c.cites = vec!["issue:#742".to_string()];
        let v = evaluate(&[c], &none());
        assert_eq!(
            v[0].outcome,
            Outcome::Ratify {
                citation: "issue:#742".to_string()
            }
        );
    }

    #[test]
    fn product_and_commercial_and_spend_keys_are_held() {
        for key in ["product.pricing", "commercial.tier", "spend.daily_cap"] {
            let mut c = cand(key, "because the operator said so", 1);
            c.cites = vec!["operator:2026-09-03".to_string()];
            let v = evaluate(&[c], &none());
            assert!(!v[0].is_ratify(), "{key} must be held even when cited");
            let (_, why) = v[0].columns();
            assert!(why.contains("held domain"), "{key}: {why}");
        }
    }

    #[test]
    fn decision_named_by_a_later_reason_is_held() {
        let mut old = cand("review.engine", "cited in issue #900", 1);
        old.cites = vec!["issue:#900".to_string()];
        let newer = cand("review.pool", "replaces review.engine after the window", 2);
        let v = evaluate(&[old, newer], &none());
        let held = v.iter().find(|x| x.key == "review.engine").unwrap();
        assert!(!held.is_ratify());
        assert!(held.columns().1.contains("superseded"), "{held:?}");
    }

    #[test]
    fn supersede_only_looks_forward() {
        // The older decision naming a key that is written later must not
        // hold the later one: that would invert the arrow.
        let older = cand("a.one", "groundwork for b.two", 1);
        let mut newer = cand("b.two", "builds on it", 2);
        newer.cites = vec!["issue:#5".to_string()];
        let v = evaluate(&[older, newer], &none());
        assert!(v.iter().find(|x| x.key == "b.two").unwrap().is_ratify());
    }

    #[test]
    fn uncited_decision_is_held() {
        let v = evaluate(&[cand("cache.ttl", "seems about right", 1)], &none());
        assert!(!v[0].is_ratify());
        assert!(v[0].columns().1.contains("no citation"));
    }

    #[test]
    fn already_ratified_decisions_are_not_reported() {
        let mut c = cand("db.engine", "issue #1", 1);
        c.ratified = true;
        assert!(evaluate(&[c], &none()).is_empty());
    }

    #[test]
    fn reason_fallback_reads_issue_numbers() {
        let v = evaluate(&[cand("x.y", "per GH-742 the pool is fixed", 1)], &none());
        assert_eq!(
            v[0].outcome,
            Outcome::Ratify {
                citation: "issue:#742 (from reason)".to_string()
            }
        );
    }

    #[test]
    fn reason_fallback_reads_a_binding_key() {
        let binding: BTreeSet<String> = ["review.carrier".to_string()].into_iter().collect();
        let v = evaluate(&[cand("x.y", "follows review.carrier", 1)], &binding);
        assert_eq!(
            v[0].outcome,
            Outcome::Ratify {
                citation: "decision:review.carrier (from reason)".to_string()
            }
        );
    }

    #[test]
    fn reason_fallback_reads_an_operator_instruction() {
        let v = evaluate(&[cand("x.y", "操作者 2026-09-03 親裁", 1)], &none());
        assert!(v[0].is_ratify());
        assert!(v[0].columns().1.contains("operator"));
    }

    #[test]
    fn structured_cite_beats_prose() {
        let mut c = cand("x.y", "per GH-1 something", 1);
        c.cites = vec!["operator:2026-09-05".to_string()];
        assert_eq!(
            evaluate(&[c], &none())[0].outcome,
            Outcome::Ratify {
                citation: "operator:2026-09-05".to_string()
            }
        );
    }

    #[test]
    fn malformed_citations_are_rejected() {
        assert!(validate_citation("issue:#742").is_ok());
        assert!(validate_citation("operator:2026-09-03").is_ok());
        assert!(validate_citation("decision:review.carrier").is_ok());
        assert!(validate_citation("just some text").is_err());
        assert!(validate_citation("issue:").is_err());
        assert!(validate_citation("ticket:12").is_err());
    }

    #[test]
    fn malformed_cites_do_not_count_as_citation() {
        let mut c = cand("x.y", "no authority named here", 1);
        c.cites = vec!["ticket:12".to_string()];
        assert!(!evaluate(&[c], &none())[0].is_ratify());
    }

    #[test]
    fn issue_number_scanner_finds_both_forms() {
        assert_eq!(issue_number("closes #12 today"), Some("12".to_string()));
        assert_eq!(issue_number("see GH-987"), Some("987".to_string()));
        assert_eq!(issue_number("gh-3 lowercase"), Some("3".to_string()));
        assert_eq!(issue_number("no numbers here"), None);
        assert_eq!(issue_number("# 12 spaced"), None);
    }
}
