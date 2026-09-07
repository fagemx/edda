//! The one admission rule over a board claim (GH-617, GH-705, GH-1018).
//!
//! `edda claim check` and the `edda dispatch --owns` guard read the same
//! board to decide the same thing, so the rule lives here rather than inside
//! either verb's command module: neither can grow its own answer without
//! deleting this one.

use edda_bridge_claude::peers::{self, classify_session_liveness_at, ClaimEntry};

/// Session ids minted by `cmd_bridge::resolve_session_id` tier 4 for bare
/// CLI invocations (`cli-<label>`). Such a session is a one-shot process:
/// it wrote its claim and exited, and no hook ever refreshes a heartbeat
/// for it, so heartbeat age carries no liveness information (GH-705). The
/// same shape is already classified in `cmd_bridge` when it names the
/// actor of a `cli-*` session.
pub(crate) fn is_bare_cli_session(session_id: &str) -> bool {
    session_id.starts_with("cli-")
}

/// Why a board claim does — or does not — still stand against a writer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ClaimStanding {
    /// A live heartbeat says the claimant is still here (GH-617).
    Live,
    /// No heartbeat can judge it, and its claim is inside the guard's TTL.
    BareWithinTtl,
    /// Nothing keeps it standing: it no longer refuses a writer.
    Expired,
}

/// The one admission rule over a board claim.
///
/// `edda claim check` and the `edda dispatch --owns` guard must answer this
/// identically — they read the same board to decide the same thing. Before
/// GH-1018 they did, by coincidence: two separate expressions that both came
/// out to `live || cli-*`. Bounding the bare-CLI arm in time would have turned
/// that coincidence into a disagreement, where `claim check` reports CONFLICT
/// on a surface `dispatch` has already admitted a writer to. One function is
/// what keeps them honest; the variants exist so `claim check` can still say
/// *why* a claim stands.
///
/// The heartbeat is consulted first, and that order is load-bearing rather
/// than a cost choice. Deciding the bare-CLI arm from the claim record alone
/// would be one file read cheaper per claim, but it would also reclassify a
/// `cli-*` claim whose session is genuinely here — answering `BareWithinTtl`
/// where GH-705 answers `Live`. Both still refuse a writer, so the exit code
/// hides the difference; the JSON report does not, and telling "a live peer
/// holds this" apart from "nobody can judge who holds this" is the whole
/// reason `unjudgeable_claims` is a separate bucket. The read this costs is
/// the read `claim check` already did for every claim before GH-1018.
pub(crate) fn claim_standing(
    project_id: &str,
    claim: &ClaimEntry,
    now_epoch: u64,
) -> ClaimStanding {
    if classify_session_liveness_at(project_id, &claim.session_id, now_epoch).is_live() {
        return ClaimStanding::Live;
    }
    if is_bare_cli_session(&claim.session_id)
        && !peers::liveness::claim_guard_expired_at(&claim.ts, now_epoch)
    {
        return ClaimStanding::BareWithinTtl;
    }
    ClaimStanding::Expired
}
