use edda_core::guided_execution::{ControlActionKindV1, ControlStateV1};
use edda_ledger::{ControlEffectRequestV1, Ledger};

pub(crate) fn has_ambiguous_dispatch_predecessor(
    ledger: &Ledger,
    request: &ControlEffectRequestV1,
) -> anyhow::Result<bool> {
    Ok(ledger
        .control_receipts(&request.manifest.control_id)?
        .into_iter()
        .any(|receipt| {
            receipt.action_kind == ControlActionKindV1::DispatchTask
                && receipt.next_state == ControlStateV1::NeedsDecision
                && receipt.dispatch_handle.is_some()
        }))
}
