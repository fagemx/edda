use serde::{Deserialize, Serialize};

pub const CONTINUITY_CAPSULE_VERSION: u8 = 1;
pub const CONTINUITY_BUNDLE_VERSION: u8 = 1;
pub const CONTINUITY_RECORD_VERSION: u8 = 1;
pub const CONTINUITY_EVENT_TYPE: &str = "checkpoint";
pub const MAX_CONTINUITY_INPUT_BYTES: usize = 256 * 1024;
pub const MAX_CONTINUITY_BUNDLE_BYTES: usize = 512 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ContextCapsuleInputV1 {
    pub capsule_version: u8,
    pub state: ContextStateInputV1,
    #[serde(default)]
    pub references: ContextReferencesV1,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ContextSourceV1 {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub machine_alias: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actor: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ContextStateInputV1 {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub goal: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current: Option<String>,
    #[serde(default)]
    pub hypotheses: Vec<String>,
    #[serde(default)]
    pub rejected: Vec<RejectedContextHypothesisV1>,
    #[serde(default)]
    pub open_questions: Vec<String>,
    pub next_action: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct RejectedContextHypothesisV1 {
    pub hypothesis: String,
    pub reason: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ContextReferencesV1 {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub task_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub event_ids: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ContextCapsuleV1 {
    pub capsule_version: u8,
    pub capsule_id: String,
    pub created_at: String,
    pub source: ContextSourceV1,
    pub repository: CapsuleRepositoryV1,
    pub state: ContextStateV1,
    pub git: CapsuleGitV1,
    pub references: ContextReferencesV1,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub truncation: Vec<TruncationNoticeV1>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct CapsuleRepositoryV1 {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub portable_repo_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_hint: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub local_only_reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ContextStateV1 {
    pub title: String,
    pub summary: String,
    pub goal: String,
    pub current: String,
    pub hypotheses: Vec<String>,
    pub rejected: Vec<RejectedContextHypothesisV1>,
    pub open_questions: Vec<String>,
    pub next_action: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct CapsuleGitV1 {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub head_sha: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detached: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tree_dirty: Option<bool>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub dirty_paths: Vec<String>,
    pub dirty_paths_truncated: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct TruncationNoticeV1 {
    pub field: String,
    pub omitted_chars: usize,
    pub omitted_items: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DataAuthority {
    DataOnly,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct CapsuleOriginV1 {
    pub capsule_id: String,
    pub event_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub portable_repo_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct CapsuleRecordV1 {
    pub record_version: u8,
    pub data_authority: DataAuthority,
    pub origin: CapsuleOriginV1,
    pub capsule_sha256: String,
    pub capsule_bytes_hex: String,
    pub capsule: ContextCapsuleV1,
    pub imported: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct PortableCapsuleBundleV1 {
    pub bundle_version: u8,
    pub portable_repo_id: String,
    pub origin_capsule_id: String,
    pub origin_event_id: String,
    pub capsule_sha256: String,
    pub capsule_bytes_hex: String,
    pub bundle_sha256: String,
    pub data_authority: DataAuthority,
}
