//! `--evidence pr#<N>@<sha>` — the typed evidence form of ratification
//! (GH-764).
//!
//! "A merged PR made this decision binding" is a fact with two parts, so it
//! is parsed into two parts here instead of living as a hand-spelled string
//! in whatever caller happens to write it (the debt #766 D8 names). The
//! binary owns the shape; a caller that gets it wrong is rejected at the
//! boundary with exit 2, not recorded as an unparseable `--by`.

use std::fmt;

/// A merged pull request and the exact commit it merged.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Evidence {
    pub pr: u64,
    /// Full 40-hex commit SHA, lowercased. Never abbreviated: an evidence
    /// trail that cannot be resolved back to one commit is not evidence.
    pub sha: String,
}

impl fmt::Display for Evidence {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "pr#{}@{}", self.pr, self.sha)
    }
}

impl Evidence {
    /// The `ratified_by` value this evidence writes: `evidence:pr#N@sha`.
    ///
    /// The `evidence:` prefix is what makes the form distinguishable in
    /// `edda log` and `edda ask` from a rule ratification (`rule:<name>`,
    /// GH-761) and from an operator's free-text `--by`.
    pub fn ratified_by(&self) -> String {
        format!("evidence:{self}")
    }
}

/// Parse `pr#<N>@<40-hex>`. The error is a usage message, not a diagnosis:
/// the caller turns it into exit 2.
pub fn parse_evidence(raw: &str) -> Result<Evidence, String> {
    let s = raw.trim();
    let rest = s
        .strip_prefix("pr#")
        .ok_or_else(|| format!("evidence must start with 'pr#' — got '{raw}'"))?;
    let (num, sha) = rest
        .split_once('@')
        .ok_or_else(|| format!("evidence must be 'pr#<N>@<40-hex sha>' — got '{raw}'"))?;
    if num.is_empty() || !num.bytes().all(|b| b.is_ascii_digit()) {
        return Err(format!("evidence PR number must be digits — got '{num}'"));
    }
    let pr: u64 = num
        .parse()
        .map_err(|_| format!("evidence PR number out of range — got '{num}'"))?;
    if sha.len() != 40 || !sha.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(format!(
            "evidence needs a full 40-hex commit SHA — got '{sha}' ({} chars)",
            sha.len()
        ));
    }
    Ok(Evidence {
        pr,
        sha: sha.to_ascii_lowercase(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const SHA: &str = "03c604ffea4b2a1731b7866e7f701374eb03b156";

    #[test]
    fn parses_pr_and_full_sha() {
        let e = parse_evidence(&format!("pr#764@{SHA}")).unwrap();
        assert_eq!(e.pr, 764);
        assert_eq!(e.sha, SHA);
        assert_eq!(e.ratified_by(), format!("evidence:pr#764@{SHA}"));
    }

    #[test]
    fn uppercase_sha_is_normalized() {
        let e = parse_evidence(&format!("pr#1@{}", SHA.to_ascii_uppercase())).unwrap();
        assert_eq!(e.sha, SHA);
    }

    #[test]
    fn abbreviated_sha_is_rejected() {
        // The whole point of the typed form: a 7-char SHA cannot be resolved
        // back to one commit years later.
        assert!(parse_evidence("pr#764@03c604f").is_err());
    }

    #[test]
    fn missing_pr_prefix_is_rejected() {
        assert!(parse_evidence(&format!("764@{SHA}")).is_err());
    }

    #[test]
    fn missing_at_is_rejected() {
        assert!(parse_evidence("pr#764").is_err());
    }

    #[test]
    fn non_hex_sha_is_rejected() {
        assert!(parse_evidence("pr#764@zzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz").is_err());
    }

    #[test]
    fn non_numeric_pr_is_rejected() {
        assert!(parse_evidence(&format!("pr#abc@{SHA}")).is_err());
    }

    #[test]
    fn surrounding_whitespace_is_tolerated() {
        assert!(parse_evidence(&format!("  pr#12@{SHA}  ")).is_ok());
    }
}
