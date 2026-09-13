pub mod blob_meta;
pub mod blob_store;
pub mod continuity;
mod control;
mod control_authority;
mod control_events;
mod control_projection;
mod control_projection_targets;
mod control_review_artifact;
mod control_review_claim;
pub mod device_token;
pub mod domain;
pub mod guided_execution;
pub mod ledger;
pub mod lock;
pub mod paths;
pub(crate) mod sqlite_store;
pub mod sync;
pub mod task_actions;
pub mod tasks;
pub mod tombstone;
pub mod verdict;
pub mod view;

#[cfg(test)]
mod control_tests;

pub use blob_meta::{BlobClass, BlobMetaEntry, BlobMetaMap, ClassChange};
pub use blob_store::{
    blob_archive, blob_get_path, blob_is_archived, blob_list, blob_list_archived,
    blob_put_classified, blob_put_if_large, blob_remove, blob_size, BlobInfo,
    SNAPSHOT_BLOB_THRESHOLD,
};
pub use continuity::{CapsuleEntryV1, ContinuityReadbackError, ImportDisposition, ImportResultV1};
pub use control::{CompiledControlV1, ControlEffectRequestV1, ControlEffectResultV1};
pub use control_authority::{
    read_owner_only_file, ControlAuthorityProvision, ControlMergeCapabilityProvision,
    ProvisionedControlAuthorityV1, VerifiedControlMergeCapabilityV1,
};
pub use control_review_claim::ControlReviewClaimOutcomeV1;
pub use domain::{
    BundleRow, ChainEntryView, DayCount, DecideSnapshotRow, DependencyEdge, DetectedPattern,
    DeviceTokenRow, DomainCount, ExecutionLinked, ImportParams, OutcomeMetrics,
    PatternDetectionResult, PatternType, RatificationInfo, SuggestionRow, TaskBriefRow, TaskLease,
    VillageStats, VillageStatsPeriod,
};
pub use guided_execution::{AcceptedExecutionBriefV1, ExecutionBriefReadbackError};
pub use ledger::Ledger;
pub use lock::WorkspaceLock;
pub use paths::{validate_branch_name, EddaPaths};
pub use sqlite_store::{UnsupportedSchemaVersionError, MAX_KNOWN_SCHEMA_VERSION};
pub use tasks::{TaskStatus, TaskView};
pub use tombstone::{append_tombstone, list_tombstones, make_tombstone, DeleteReason, Tombstone};
pub use verdict::{latest_verdict, parse_verdict_event, VerdictRecord};
pub use view::DecisionView;

/// Maximum number of decision snapshots returned by one query.
pub const MAX_SNAPSHOT_QUERY_LIMIT: usize = 100;
