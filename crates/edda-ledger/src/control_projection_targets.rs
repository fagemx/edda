use super::control_projection::ProjectedControl;
use crate::Ledger;
use edda_core::guided_execution::{
    ControlActionKindV1, ControlStateV1, ControlTargetV1, ControlTaskV1, WorkReceiptV1,
};

pub(super) fn task_target(
    ledger: &Ledger,
    task: &ControlTaskV1,
) -> anyhow::Result<ControlTargetV1> {
    let attempt = match task.task_id {
        Some(task_id) => ledger
            .task_views()?
            .into_iter()
            .find(|view| view.task_id == task_id)
            .map_or(1, |view| match view.status {
                crate::TaskStatus::Ready | crate::TaskStatus::Failed => view.attempts + 1,
                _ => view.attempts.max(1),
            }),
        None => 1,
    };
    Ok(ControlTargetV1::Task {
        task_key: task.task_key.clone(),
        attempt,
    })
}

pub(super) fn delivery_target(
    ledger: &Ledger,
    task: &ControlTaskV1,
) -> anyhow::Result<Option<ControlTargetV1>> {
    let Some(task_id) = task.task_id else {
        return Ok(None);
    };
    let Some(view) = ledger
        .task_views()?
        .into_iter()
        .find(|view| view.task_id == task_id)
    else {
        return Ok(None);
    };
    if view.status != crate::TaskStatus::Done {
        return Ok(None);
    }
    let receipt: WorkReceiptV1 = serde_json::from_str(
        view.receipt
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!("completed controlled task omits its receipt"))?,
    )
    .map_err(|_| anyhow::anyhow!("completed controlled task receipt is malformed"))?;
    Ok(receipt.delivery.map(|delivery| ControlTargetV1::Delivery {
        task_key: task.task_key.clone(),
        attempt: view.attempts,
        portable_repo_id: delivery.portable_repo_id,
        branch: delivery.branch,
        result_head_sha: delivery.result_head_sha,
        pr_number: delivery.pr_number,
        pr_base_ref: delivery.pr_base_ref,
        observed_base_tip_sha: delivery.observed_base_tip_sha,
    }))
}

pub(super) fn bound_pr_target(projected: &ProjectedControl) -> anyhow::Result<ControlTargetV1> {
    let expected_pr = projected.record.manifest.merge_policy.pr_number;
    let expected_head = projected
        .record
        .manifest
        .merge_policy
        .expected_head_sha
        .as_deref();
    let mut candidates = Vec::new();
    for (_, receipt) in projected.receipts.values() {
        if receipt.next_state == ControlStateV1::NeedsDecision
            || receipt.action_kind != ControlActionKindV1::BindDelivery
        {
            continue;
        }
        let ControlTargetV1::Delivery {
            pr_number: Some(number),
            result_head_sha,
            ..
        } = &receipt.target
        else {
            continue;
        };
        candidates.push((*number, result_head_sha.clone()));
    }
    candidates.sort();
    candidates.dedup();
    anyhow::ensure!(
        candidates.len() <= 1,
        "control has multiple independently bound delivery PR subjects"
    );
    if let Some((number, head_sha)) = candidates.pop() {
        anyhow::ensure!(
            expected_pr.is_none_or(|expected| expected == number)
                && expected_head.is_none_or(|expected| expected == head_sha),
            "bound delivery PR subject differs from the manifest expectation"
        );
        return Ok(ControlTargetV1::PullRequest { number, head_sha });
    }
    Ok(ControlTargetV1::PullRequest {
        number: expected_pr.ok_or_else(|| anyhow::anyhow!("control has no bound PR number"))?,
        head_sha: expected_head
            .map(str::to_owned)
            .ok_or_else(|| anyhow::anyhow!("control has no bound PR head"))?,
    })
}
