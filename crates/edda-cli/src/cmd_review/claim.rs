use super::github::{
    controlled_gh_capture_for, controlled_gh_for, ControlledGitHubTransport, GitHubRepository,
};
use anyhow::{Context, Result};
pub(crate) use edda_core::guided_execution::ControlReviewClaimV1 as StructuredReviewRequest;
use edda_ledger::{ControlReviewClaimOutcomeV1, Ledger};
use std::path::Path;

const CLAIM_TAG_NAMESPACE: &str = "edda/review-claims";

#[derive(Debug, Clone)]
pub(crate) enum ReviewClaimOutcome {
    Won {
        adopted: bool,
        external_identity: String,
        request: Box<StructuredReviewRequest>,
    },
    Refused {
        reason_code: String,
    },
}

trait RemoteClaimEvidence {
    /// Create immutable GitHub evidence. It is never the live-claim authority.
    fn create_if_absent(&self, request: &StructuredReviewRequest) -> Result<bool>;
    fn read(&self, request: &StructuredReviewRequest) -> Result<Option<StructuredReviewRequest>>;
}

trait LocalClaimAuthority {
    fn claim(&self, request: &StructuredReviewRequest) -> Result<LocalClaimOutcome>;
    fn verify(&self, request: &StructuredReviewRequest) -> Result<()>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LocalClaimOutcome {
    Won { adopted: bool },
    RefusedLive,
}

struct GitHubClaimEvidence<'a> {
    transport: &'a ControlledGitHubTransport,
    checkout: &'a Path,
    repository: &'a GitHubRepository,
}

struct LedgerClaimAuthority<'a> {
    ledger: &'a Ledger,
}

fn claim_ref(request: &StructuredReviewRequest) -> String {
    // The authority seal makes the generation suffix unpredictable before the
    // host-local claim is signed. A repository writer can leave conflicting
    // evidence, but cannot pre-create the expected ref or forge its payload.
    format!(
        "refs/tags/{CLAIM_TAG_NAMESPACE}/pr-{}/{}/g-{}-{}",
        request.pr_number,
        request.head_sha,
        request.generation,
        &request.seal[..20]
    )
}

fn api_ref(request: &StructuredReviewRequest) -> String {
    format!(
        "tags/{CLAIM_TAG_NAMESPACE}/pr-{}/{}/g-{}-{}",
        request.pr_number,
        request.head_sha,
        request.generation,
        &request.seal[..20]
    )
}

impl RemoteClaimEvidence for GitHubClaimEvidence<'_> {
    fn create_if_absent(&self, request: &StructuredReviewRequest) -> Result<bool> {
        let message = serde_json::to_string(request)?;
        let tag_name = format!(
            "edda-review-claim-g{}-{}",
            request.generation,
            &request.seal[..20]
        );
        let tag = controlled_gh_for(
            self.transport,
            self.checkout,
            self.repository,
            &[
                "api",
                "--method",
                "POST",
                "repos/{owner}/{repo}/git/tags",
                "-f",
                &format!("tag={tag_name}"),
                "-f",
                &format!("message={message}"),
                "-f",
                &format!("object={}", request.head_sha),
                "-f",
                "type=commit",
            ],
        )?;
        let tag_sha = tag["sha"]
            .as_str()
            .context("GitHub tag-object response omits SHA")?;
        validate_hex(tag_sha, &[40, 64], "claim tag object")?;
        let output = controlled_gh_capture_for(
            self.transport,
            self.checkout,
            self.repository,
            &[
                "api",
                "--method",
                "POST",
                "repos/{owner}/{repo}/git/refs",
                "-f",
                &format!("ref={}", claim_ref(request)),
                "-f",
                &format!("sha={tag_sha}"),
            ],
        )?;
        // GitHub create-ref is immutable evidence only. The local projection
        // already selected the live winner atomically before this call.
        Ok(output.status.success())
    }

    fn read(&self, request: &StructuredReviewRequest) -> Result<Option<StructuredReviewRequest>> {
        let reference = match controlled_gh_for(
            self.transport,
            self.checkout,
            self.repository,
            &[
                "api",
                &format!("repos/{{owner}}/{{repo}}/git/ref/{}", api_ref(request)),
            ],
        ) {
            Ok(value) => value,
            Err(_) => return Ok(None),
        };
        anyhow::ensure!(
            reference["ref"].as_str() == Some(claim_ref(request).as_str())
                && reference["object"]["type"].as_str() == Some("tag"),
            "review claim evidence reference has an unexpected shape"
        );
        let tag_sha = reference["object"]["sha"]
            .as_str()
            .context("review claim evidence omits tag object")?;
        validate_hex(tag_sha, &[40, 64], "claim tag object")?;
        let tag = controlled_gh_for(
            self.transport,
            self.checkout,
            self.repository,
            &[
                "api",
                &format!("repos/{{owner}}/{{repo}}/git/tags/{tag_sha}"),
            ],
        )?;
        anyhow::ensure!(
            tag["object"]["type"].as_str() == Some("commit")
                && tag["object"]["sha"].as_str() == Some(request.head_sha.as_str()),
            "review claim evidence tag is not bound to the requested head"
        );
        let parsed: StructuredReviewRequest = serde_json::from_str(
            tag["message"]
                .as_str()
                .context("review claim evidence tag omits structured request")?,
        )
        .context("review claim evidence request is malformed")?;
        Ok(Some(parsed))
    }
}

impl LocalClaimAuthority for LedgerClaimAuthority<'_> {
    fn claim(&self, request: &StructuredReviewRequest) -> Result<LocalClaimOutcome> {
        Ok(match self.ledger.claim_control_review(request)? {
            ControlReviewClaimOutcomeV1::Won { adopted } => LocalClaimOutcome::Won { adopted },
            ControlReviewClaimOutcomeV1::RefusedLive => LocalClaimOutcome::RefusedLive,
        })
    }

    fn verify(&self, request: &StructuredReviewRequest) -> Result<()> {
        self.ledger.verify_control_review_claim(request)
    }
}

pub(crate) fn claim(
    transport: &ControlledGitHubTransport,
    checkout: &Path,
    ledger: &Ledger,
    repository: &GitHubRepository,
    request: StructuredReviewRequest,
) -> Result<ReviewClaimOutcome> {
    edda_core::guided_execution::validate_control_review_claim(&request)?;
    ledger.verify_control_review_claim(&request)?;
    let observed =
        super::github::pr_delivery_subject_for(transport, checkout, repository, request.pr_number)?;
    if observed.head_sha != request.head_sha
        || observed.base_sha != request.base_sha
        || observed.state != "OPEN"
    {
        return Ok(ReviewClaimOutcome::Refused {
            reason_code: "REVIEW_SUBJECT_MOVED".into(),
        });
    }
    claim_with_stores(
        &LedgerClaimAuthority { ledger },
        &GitHubClaimEvidence {
            transport,
            checkout,
            repository,
        },
        request,
    )
}

fn claim_with_stores(
    local: &dyn LocalClaimAuthority,
    remote: &dyn RemoteClaimEvidence,
    request: StructuredReviewRequest,
) -> Result<ReviewClaimOutcome> {
    let adopted = match local.claim(&request)? {
        LocalClaimOutcome::Won { adopted } => adopted,
        LocalClaimOutcome::RefusedLive => {
            return Ok(ReviewClaimOutcome::Refused {
                reason_code: "REVIEW_CLAIM_LIVE".into(),
            });
        }
    };
    let created = match remote.create_if_absent(&request) {
        Ok(created) => created,
        Err(_) => {
            return Ok(ReviewClaimOutcome::Refused {
                reason_code: "REVIEW_CLAIM_EVIDENCE_UNAVAILABLE".into(),
            });
        }
    };
    let winner = match remote.read(&request) {
        Ok(Some(winner)) => winner,
        Ok(None) => {
            return Ok(ReviewClaimOutcome::Refused {
                reason_code: "REVIEW_CLAIM_EVIDENCE_UNAVAILABLE".into(),
            });
        }
        Err(_) => {
            return Ok(ReviewClaimOutcome::Refused {
                reason_code: "REVIEW_CLAIM_EVIDENCE_INVALID".into(),
            });
        }
    };
    if local.verify(&winner).is_err() || winner != request {
        return Ok(ReviewClaimOutcome::Refused {
            reason_code: "REVIEW_CLAIM_EVIDENCE_INVALID".into(),
        });
    }
    Ok(ReviewClaimOutcome::Won {
        adopted: adopted || !created,
        external_identity: claim_ref(&request),
        request: Box::new(request),
    })
}

fn validate_hex(value: &str, lengths: &[usize], field: &str) -> Result<()> {
    anyhow::ensure!(lengths.contains(&value.len()), "{field} has invalid length");
    anyhow::ensure!(
        value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase()),
        "{field} is not lowercase hexadecimal"
    );
    Ok(())
}

#[cfg(test)]
#[path = "claim_tests.rs"]
mod tests;
