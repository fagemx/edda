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
    /// Ledger timestamp (RFC3339), for `--since` bounding (GH-1066). Empty
    /// when unknown. `evaluate` never reads this field — `--since` filters
    /// the caller's output (`mod.rs::sweep`), not a verdict here, so an
    /// empty date never changes what this pure function decides.
    pub date: String,
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
    // Supersession is explicit, never inferred from a mention (GH-1066): a
    // decision that merely comes up while explaining something else has not
    // overtaken it. Only two things count as "superseded" now, checked in
    // this order because same-key is unambiguous and free, and an explicit
    // marker is still cheaper to trust than reading the domain guard below.
    if same_key_superseded(c, all) {
        return Outcome::Hold {
            why: format!(
                "superseded-by-same-key — a later decision recorded under '{}' replaces it",
                c.key
            ),
        };
    }
    if let Some(newer) = all
        .iter()
        .filter(|o| o.order > c.order)
        .find(|o| names_explicit_supersession(&o.reason, &c.key))
    {
        return Outcome::Hold {
            why: format!("superseded-explicit — '{}' marks it superseded", newer.key),
        };
    }
    // Domain guard (GH-1066): a valid citation is not immunity. A newer,
    // *binding* ruling in the same governance domain can have overtaken this
    // one in substance even though it never names it — decision.auto-ratify's
    // "contradicts no binding decision" clause, approximated the only way
    // that does not require reading prose.
    if let Some(later) = domain_superseded_by(c, all) {
        return Outcome::Hold {
            why: format!(
                "older-than-binding-in-domain — '{later}' is binding and newer in domain '{}'",
                domain_of(&c.key)
            ),
        };
    }
    match citation(c, binding_keys) {
        Some(citation) => Outcome::Ratify { citation },
        None => Outcome::Hold {
            why: "no-citation — add --cite operator:<when> | issue:#<n> | decision:<key>"
                .to_string(),
        },
    }
}

/// True when a later decision was recorded under the identical key — the
/// most explicit supersession there is: nothing needs inferring when the
/// ledger already carries a newer value for the same key.
fn same_key_superseded(c: &Candidate, all: &[Candidate]) -> bool {
    all.iter().any(|o| o.key == c.key && o.order > c.order)
}

/// Marker tokens this rule reads as an *explicit* supersession claim in a
/// decision's own reason — never inferred from a mention anywhere in the
/// prose, only from the target key appearing immediately after one of these
/// tokens. `supersedes:` is the canonical form this rule writes going
/// forward (GH-1066 doneWhen); `SUPERSEDES` (bare, all-caps) and `顯式取代`
/// are phrasings already on the ledger before this rule existed — recognised
/// so existing decisions need no migration, and never rewritten.
const SUPERSEDES_MARKERS: [&str; 3] = ["supersedes:", "SUPERSEDES", "顯式取代"];

/// True when `reason` explicitly names `target_key` as superseded: one of
/// `SUPERSEDES_MARKERS`, immediately followed (after optional separator
/// whitespace or a colon) by `target_key` at a word boundary. `target_key`
/// appearing anywhere else in the sentence does not count — that is the
/// exact shape of the GH-1066 bug this replaces.
///
/// Checks **every** occurrence of each marker, not only the first (PR #1098
/// Round 1 P2): a reason superseding two keys with the same marker —
/// `"supersedes:a.one and supersedes:b.two"` — must hold both, and a
/// single `str::find` only ever sees the first one.
fn names_explicit_supersession(reason: &str, target_key: &str) -> bool {
    SUPERSEDES_MARKERS.iter().any(|marker| {
        reason.match_indices(marker).any(|(idx, _)| {
            let after = reason[idx + marker.len()..].trim_start_matches([' ', ':', '\t', '　']);
            after.strip_prefix(target_key).is_some_and(|rest| {
                rest.chars()
                    .next()
                    .is_none_or(|ch| !(ch.is_alphanumeric() || matches!(ch, '_' | '-' | '.')))
            })
        })
    })
}

/// The domain segment of a key: everything before the first `.` — the same
/// split the ledger uses to populate `decisions.domain`.
fn domain_of(key: &str) -> &str {
    key.split('.').next().unwrap_or(key)
}

/// The key of a later, **binding** decision in the same domain, if any
/// (GH-1066 domain guard). Only a ratified sibling counts: an unratified
/// newer decision in the same domain has no more authority than `c` itself
/// yet, so it cannot be what overtook it.
fn domain_superseded_by(c: &Candidate, all: &[Candidate]) -> Option<String> {
    let domain = domain_of(&c.key);
    all.iter()
        .filter(|o| o.ratified && o.order > c.order && o.key != c.key)
        .find(|o| domain_of(&o.key) == domain)
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
            date: String::new(),
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
    fn mention_alone_no_longer_holds_it() {
        // GH-1066: this is the exact shape of the bug. `review.pool`'s
        // reason names `review.engine` in passing (it builds on it, it does
        // not replace it), so a later decision *mentioning* an earlier key
        // must not hold it — only a same-key redecision or an explicit
        // marker does. `review.engine` has a real citation and nothing
        // superseded it, so it ratifies.
        let mut old = cand("review.engine", "cited in issue #900", 1);
        old.cites = vec!["issue:#900".to_string()];
        let newer = cand("review.pool", "replaces review.engine after the window", 2);
        let v = evaluate(&[old, newer], &none());
        let engine = v.iter().find(|x| x.key == "review.engine").unwrap();
        assert!(engine.is_ratify(), "{engine:?}");
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

    // ── same-key supersession (GH-1066) ─────────────────────────────────

    #[test]
    fn same_key_later_decision_holds_the_older_row() {
        let mut old = cand("db.engine", "postgres, per #1", 1);
        old.cites = vec!["issue:#1".to_string()];
        let mut newer = cand("db.engine", "sqlite instead, embedded is simpler", 2);
        newer.cites = vec!["issue:#2".to_string()];
        let v = evaluate(&[old, newer], &none());
        // Both verdicts share the key "db.engine" here — same-key chains are
        // exactly the case where key alone cannot disambiguate — so pick out
        // each side by outcome instead. Both rows carry a valid citation, so
        // only same-key supersession explains why one is held.
        assert_eq!(v.len(), 2);
        let held = v.iter().find(|x| !x.is_ratify()).unwrap();
        assert!(
            held.columns().1.starts_with("superseded-by-same-key"),
            "{held:?}"
        );
        assert!(v.iter().any(|x| x.is_ratify()), "{v:?}");
    }

    #[test]
    fn newest_row_in_a_same_key_chain_still_reaches_citation() {
        let old = cand("db.engine", "postgres", 1);
        let mut newer = cand("db.engine", "sqlite instead", 2);
        newer.cites = vec!["issue:#2".to_string()];
        let v = evaluate(&[old, newer], &none());
        // Two candidates share one key here (mod.rs::collect no longer
        // dedups before the rule sees them — GH-1066), so `evaluate` returns
        // two verdicts under the same key; the newest one must ratify.
        assert_eq!(v.len(), 2);
        assert!(v.iter().any(|x| x.is_ratify()), "{v:?}");
        assert!(v.iter().any(|x| !x.is_ratify()), "{v:?}");
    }

    // ── explicit-marker supersession (GH-1066) ──────────────────────────

    #[test]
    fn explicit_canonical_marker_holds_the_named_key() {
        let mut old = cand("review.old-gate", "cited in issue #900", 1);
        old.cites = vec!["issue:#900".to_string()];
        let newer = cand(
            "review.new-gate",
            "supersedes:review.old-gate — the gate moved to the aggregate check",
            2,
        );
        let v = evaluate(&[old, newer], &none());
        let held = v.iter().find(|x| x.key == "review.old-gate").unwrap();
        assert!(!held.is_ratify());
        assert!(
            held.columns().1.starts_with("superseded-explicit"),
            "{held:?}"
        );
    }

    #[test]
    fn legacy_all_caps_supersedes_marker_is_recognized() {
        // The exact phrasing already on the ledger before this rule existed
        // (fleet.merge-authority's own reason text): `SUPERSEDES <key>` with
        // no colon. Recognised, not migrated.
        let mut old = cand("fleet.old-policy", "per #12", 1);
        old.cites = vec!["issue:#12".to_string()];
        let newer = cand(
            "fleet.new-policy",
            "SUPERSEDES fleet.old-policy=old-value ONLY in its gate clause",
            2,
        );
        let v = evaluate(&[old, newer], &none());
        let held = v.iter().find(|x| x.key == "fleet.old-policy").unwrap();
        assert!(!held.is_ratify());
        assert!(
            held.columns().1.starts_with("superseded-explicit"),
            "{held:?}"
        );
    }

    #[test]
    fn legacy_chinese_explicit_marker_is_recognized() {
        let mut old = cand("review.old-carrier", "per #7", 1);
        old.cites = vec!["issue:#7".to_string()];
        let newer = cand(
            "review.new-carrier",
            "顯式取代review.old-carrier，理由如下",
            2,
        );
        let v = evaluate(&[old, newer], &none());
        let held = v.iter().find(|x| x.key == "review.old-carrier").unwrap();
        assert!(!held.is_ratify());
        assert!(
            held.columns().1.starts_with("superseded-explicit"),
            "{held:?}"
        );
    }

    #[test]
    fn explicit_marker_used_twice_holds_both_named_keys() {
        // PR #1098 Round 1 P2: `reason.find(marker)` located only the first
        // "supersedes:" occurrence, so a reason superseding two keys with
        // the same marker held only the first-named one. `match_indices`
        // checks every occurrence.
        let mut a = cand("a.one", "cited a", 1);
        a.cites = vec!["issue:#1".to_string()];
        let mut b = cand("b.two", "cited b", 2);
        b.cites = vec!["issue:#2".to_string()];
        let newer = cand("c.three", "supersedes:a.one and supersedes:b.two", 3);
        let v = evaluate(&[a, b, newer], &none());
        for key in ["a.one", "b.two"] {
            let held = v.iter().find(|x| x.key == key).unwrap();
            assert!(!held.is_ratify(), "{key}: {held:?}");
            assert!(
                held.columns().1.starts_with("superseded-explicit"),
                "{key}: {held:?}"
            );
        }
    }

    #[test]
    fn explicit_marker_key_must_be_a_whole_word_not_a_prefix() {
        // `review.merge-gate` must not be caught by a marker that actually
        // names the longer key `review.merge-gate-v2` — the whole point of
        // GH-1066 is that a partial textual match never counts as explicit.
        let mut short = cand("review.merge-gate", "per #55", 1);
        short.cites = vec!["issue:#55".to_string()];
        let newer = cand(
            "review.merge-gate-v2",
            "supersedes:review.merge-gate-v2 — restates the v1 record",
            2,
        );
        let v = evaluate(&[short, newer], &none());
        let short_verdict = v.iter().find(|x| x.key == "review.merge-gate").unwrap();
        assert!(short_verdict.is_ratify(), "{short_verdict:?}");
    }

    // ── domain guard (GH-1066) ───────────────────────────────────────────

    #[test]
    fn domain_guard_holds_older_candidate_behind_a_later_binding_sibling() {
        let mut old = cand("review.legacy-gate", "per #100", 1);
        old.cites = vec!["issue:#100".to_string()];
        let mut newer = cand("review.auto-merge", "gate is now mechanical, per Tim", 2);
        newer.ratified = true;
        let v = evaluate(&[old, newer], &none());
        let held = v.iter().find(|x| x.key == "review.legacy-gate").unwrap();
        assert!(!held.is_ratify());
        assert!(
            held.columns().1.starts_with("older-than-binding-in-domain"),
            "{held:?}"
        );
    }

    #[test]
    fn domain_guard_does_not_cross_domains() {
        let mut old = cand("fleet.legacy-gate", "per #100", 1);
        old.cites = vec!["issue:#100".to_string()];
        let mut newer = cand("review.auto-merge", "gate is now mechanical", 2);
        newer.ratified = true;
        let v = evaluate(&[old, newer], &none());
        let verdict = v.iter().find(|x| x.key == "fleet.legacy-gate").unwrap();
        assert!(verdict.is_ratify(), "{verdict:?}");
    }

    #[test]
    fn domain_guard_requires_the_sibling_to_be_binding_not_merely_newer() {
        // An unratified newer decision in the same domain has no more
        // authority than the candidate itself — it must not hold it.
        let mut old = cand("review.legacy-gate", "per #100", 1);
        old.cites = vec!["issue:#100".to_string()];
        let newer = cand("review.auto-merge", "gate is now mechanical", 2); // not ratified
        let v = evaluate(&[old, newer], &none());
        let verdict = v.iter().find(|x| x.key == "review.legacy-gate").unwrap();
        assert!(verdict.is_ratify(), "{verdict:?}");
    }

    // The two tests above prove the domain guard on `review.` keys sharing a
    // domain. PR #1098 Round 2 (controller amendment to GH-1066 doneWhen
    // bullet 5, 2026-09-09) requires the same positive/negative pair proven
    // on a second, genuinely shared domain — `fleet.` — rather than relying
    // on `domain_guard_does_not_cross_domains` below, which is a cross-domain
    // negative and proves a different property.

    #[test]
    fn domain_guard_holds_older_fleet_candidate_behind_a_later_binding_fleet_sibling() {
        let mut old = cand("fleet.legacy-gate", "per #100", 1);
        old.cites = vec!["issue:#100".to_string()];
        let mut newer = cand("fleet.rollout-gate", "dispatch is now mechanical", 2);
        newer.ratified = true;
        let v = evaluate(&[old, newer], &none());
        let held = v.iter().find(|x| x.key == "fleet.legacy-gate").unwrap();
        assert!(!held.is_ratify());
        assert!(
            held.columns().1.starts_with("older-than-binding-in-domain"),
            "{held:?}"
        );
    }

    #[test]
    fn domain_guard_requires_the_fleet_sibling_to_be_binding_not_merely_newer() {
        let mut old = cand("fleet.legacy-gate", "per #100", 1);
        old.cites = vec!["issue:#100".to_string()];
        let newer = cand("fleet.rollout-gate", "dispatch is now mechanical", 2); // not ratified
        let v = evaluate(&[old, newer], &none());
        let verdict = v.iter().find(|x| x.key == "fleet.legacy-gate").unwrap();
        assert!(verdict.is_ratify(), "{verdict:?}");
    }

    #[test]
    fn uncited_decision_is_held() {
        let v = evaluate(&[cand("cache.ttl", "seems about right", 1)], &none());
        assert!(!v[0].is_ratify());
        assert!(v[0].columns().1.contains("no-citation"));
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
