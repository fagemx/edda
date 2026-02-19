use edda_core::event::{
    new_approval_event, new_approval_request_event, new_commit_event, ApprovalEventParams,
    ApprovalRequestParams, CommitEventParams,
};
use edda_derive::{build_auto_evidence, last_commit_contribution, rebuild_all};
use edda_ledger::lock::WorkspaceLock;
use edda_ledger::Ledger;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashSet};
use std::path::Path;

// ── Policy v2 data model ──

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct PolicyV2Config {
    pub version: u32,
    #[serde(default)]
    pub roles: Vec<String>,
    #[serde(default)]
    pub rules: Vec<PolicyRule>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct PolicyRule {
    pub id: String,
    #[serde(default)]
    pub when: PolicyWhen,
    #[serde(default)]
    pub stages: Vec<PolicyStageDef>,
}

#[derive(Debug, Serialize, Deserialize, Clone, Default)]
pub struct PolicyWhen {
    #[serde(default)]
    pub default: Option<bool>,
    #[serde(default)]
    pub labels_any: Option<Vec<String>>,
    #[serde(default)]
    pub failed_cmd: Option<bool>,
    #[serde(default)]
    pub evidence_count_gte: Option<usize>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct PolicyStageDef {
    pub stage_id: String,
    pub role: String,
    #[serde(default = "default_one")]
    pub min_approvals: usize,
    #[serde(default = "default_two")]
    pub max_assignees: usize,
}

fn default_one() -> usize {
    1
}
fn default_two() -> usize {
    2
}

// ── Actors config ──

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ActorsConfig {
    pub version: u32,
    #[serde(default)]
    pub actors: BTreeMap<String, ActorDef>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ActorDef {
    #[serde(default)]
    pub roles: Vec<String>,
}

impl Default for ActorsConfig {
    fn default() -> Self {
        Self {
            version: 1,
            actors: BTreeMap::new(),
        }
    }
}

// ── Policy v1 structs (for v1→v2 conversion) ──

#[derive(Debug, Deserialize, Clone)]
struct RequireApprovalRuleV1 {
    #[serde(default)]
    default: bool,
    #[serde(default)]
    if_labels_any: Vec<String>,
    #[serde(default)]
    if_failed_cmd: bool,
    #[serde(default)]
    if_evidence_count_gte: usize,
}

#[derive(Debug, Deserialize, Clone)]
struct PolicyV1 {
    #[serde(default = "default_min_approvals")]
    min_approvals: usize,
    #[serde(default)]
    require_approval: RequireApprovalRuleV1,
}

fn default_min_approvals() -> usize {
    1
}

impl Default for RequireApprovalRuleV1 {
    fn default() -> Self {
        Self {
            default: false,
            if_labels_any: vec!["risk".into(), "security".into(), "prod".into()],
            if_failed_cmd: true,
            if_evidence_count_gte: 15,
        }
    }
}

fn convert_v1_to_v2(v1: PolicyV1) -> PolicyV2Config {
    let r = &v1.require_approval;
    let mut when = PolicyWhen::default();
    if !r.if_labels_any.is_empty() {
        when.labels_any = Some(r.if_labels_any.clone());
    }
    if r.if_failed_cmd {
        when.failed_cmd = Some(true);
    }
    if r.if_evidence_count_gte > 0 {
        when.evidence_count_gte = Some(r.if_evidence_count_gte);
    }

    let min_app = v1.min_approvals.max(1);
    let require_rule = PolicyRule {
        id: "require".to_string(),
        when,
        stages: vec![PolicyStageDef {
            stage_id: "default".to_string(),
            role: "approver".to_string(),
            min_approvals: min_app,
            max_assignees: 0,
        }],
    };

    let default_rule = PolicyRule {
        id: "default".to_string(),
        when: PolicyWhen {
            default: Some(true),
            ..Default::default()
        },
        stages: if r.default {
            vec![PolicyStageDef {
                stage_id: "default".to_string(),
                role: "approver".to_string(),
                min_approvals: min_app,
                max_assignees: 0,
            }]
        } else {
            vec![]
        },
    };

    PolicyV2Config {
        version: 2,
        roles: vec!["approver".to_string()],
        rules: vec![require_rule, default_rule],
    }
}

// ── Policy + actors loading ──

#[derive(Deserialize)]
struct VersionCheck {
    version: u32,
}

fn load_policy_v2(ledger: &Ledger) -> anyhow::Result<PolicyV2Config> {
    let path = ledger.paths.edda_dir.join("policy.yaml");
    if !path.exists() {
        return Ok(PolicyV2Config {
            version: 2,
            roles: vec![],
            rules: vec![PolicyRule {
                id: "default".to_string(),
                when: PolicyWhen {
                    default: Some(true),
                    ..Default::default()
                },
                stages: vec![],
            }],
        });
    }
    let content = std::fs::read(&path)?;
    let vc: VersionCheck = serde_yaml::from_slice(&content)?;
    match vc.version {
        1 => {
            let v1: PolicyV1 = serde_yaml::from_slice(&content)?;
            Ok(convert_v1_to_v2(v1))
        }
        2 => {
            let v2: PolicyV2Config = serde_yaml::from_slice(&content)?;
            Ok(v2)
        }
        other => anyhow::bail!("unsupported policy version: {other}"),
    }
}

fn load_actors(ledger: &Ledger) -> anyhow::Result<ActorsConfig> {
    let path = ledger.paths.edda_dir.join("actors.yaml");
    if !path.exists() {
        return Ok(ActorsConfig::default());
    }
    let content = std::fs::read(&path)?;
    let cfg: ActorsConfig = serde_yaml::from_slice(&content)?;
    Ok(cfg)
}

// ── Route selection ──

fn evidence_has_failed_cmd_check(
    ledger: &Ledger,
    evidence: &[serde_json::Value],
) -> anyhow::Result<bool> {
    let evidence_ids: HashSet<String> = evidence
        .iter()
        .filter_map(|item| {
            item.get("event_id")
                .and_then(|x| x.as_str())
                .map(|s| s.to_string())
        })
        .collect();

    if evidence_ids.is_empty() {
        return Ok(false);
    }

    for ev in ledger.iter_events()? {
        if evidence_ids.contains(&ev.event_id) && ev.event_type == "cmd" {
            let exit_code = ev
                .payload
                .get("exit_code")
                .and_then(|x| x.as_i64())
                .unwrap_or(0);
            if exit_code != 0 {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

fn when_matches(
    when: &PolicyWhen,
    labels: &HashSet<&str>,
    has_failed_cmd: bool,
    evidence_count: usize,
) -> bool {
    if when.default == Some(true) {
        return true;
    }
    if let Some(ref la) = when.labels_any {
        if la.iter().any(|l| labels.contains(l.as_str())) {
            return true;
        }
    }
    if when.failed_cmd == Some(true) && has_failed_cmd {
        return true;
    }
    if let Some(n) = when.evidence_count_gte {
        if n > 0 && evidence_count >= n {
            return true;
        }
    }
    false
}

/// First-match route selection. Returns (rule_id, stages).
fn route_select(
    policy: &PolicyV2Config,
    labels: &[String],
    has_failed_cmd: bool,
    evidence_count: usize,
) -> (String, Vec<PolicyStageDef>) {
    let label_set: HashSet<&str> = labels.iter().map(|s| s.as_str()).collect();
    for rule in &policy.rules {
        if when_matches(&rule.when, &label_set, has_failed_cmd, evidence_count) {
            return (rule.id.clone(), rule.stages.clone());
        }
    }
    (String::new(), vec![])
}

// ── Draft data model ──

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct DraftStage {
    pub stage_id: String,
    pub role: String,
    pub min_approvals: usize,
    pub assignees: Vec<String>,
    pub status: String,
    #[serde(default)]
    pub approved_by: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ApprovalRecord {
    pub ts: String,
    pub actor: String,
    pub decision: String,
    pub note: String,
    pub approval_event_id: String,
    #[serde(default)]
    pub stage_id: String,
    #[serde(default)]
    pub role: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct CommitDraftV1 {
    pub version: u32,
    pub draft_id: String,
    pub created_at: String,
    pub branch: String,
    pub base_parent_hash: String,
    pub title: String,
    pub purpose: String,
    pub contribution: String,
    pub labels: Vec<String>,
    pub evidence: Vec<serde_json::Value>,
    pub auto_preview_lines: Vec<String>,
    pub event_preview: serde_json::Value,
    #[serde(default = "default_status")]
    pub status: String,
    #[serde(default)]
    pub approvals: Vec<ApprovalRecord>,
    #[serde(default)]
    pub applied_commit_id: String,
    #[serde(default)]
    pub policy_require_approval: bool,
    #[serde(default)]
    pub policy_min_approvals: usize,
    #[serde(default)]
    pub stages: Vec<DraftStage>,
    #[serde(default)]
    pub route_rule_id: String,
}

fn default_status() -> String {
    "proposed".to_string()
}

fn approved_count_flat(d: &CommitDraftV1) -> usize {
    d.approvals
        .iter()
        .filter(|a| a.decision == "approve")
        .count()
}

fn has_reject_flat(d: &CommitDraftV1) -> bool {
    d.approvals.iter().any(|a| a.decision == "reject")
}

fn has_stage_reject(d: &CommitDraftV1) -> bool {
    d.stages.iter().any(|s| s.status == "rejected")
}

fn all_stages_approved(d: &CommitDraftV1) -> bool {
    d.stages.iter().all(|s| s.status == "approved")
}

// ── Draft stage building ──

fn build_draft_stages(
    policy_stages: &[PolicyStageDef],
    actors: &ActorsConfig,
) -> Vec<DraftStage> {
    policy_stages
        .iter()
        .map(|ps| {
            let mut assignees: Vec<String> = actors
                .actors
                .iter()
                .filter(|(_, def)| def.roles.contains(&ps.role))
                .map(|(name, _)| name.clone())
                .collect();
            assignees.sort();
            if ps.max_assignees > 0 {
                assignees.truncate(ps.max_assignees);
            }
            DraftStage {
                stage_id: ps.stage_id.clone(),
                role: ps.role.clone(),
                min_approvals: ps.min_approvals,
                assignees,
                status: "pending".to_string(),
                approved_by: vec![],
            }
        })
        .collect()
}

// ── Helpers ──

fn new_draft_id() -> String {
    format!("drf_{}", ulid::Ulid::new().to_string().to_lowercase())
}

fn now_rfc3339() -> String {
    let now = time::OffsetDateTime::now_utc();
    now.format(&time::format_description::well_known::Rfc3339)
        .expect("RFC3339 formatting should not fail")
}

fn ensure_drafts_dir(ledger: &Ledger) -> anyhow::Result<std::path::PathBuf> {
    let dir = ledger.paths.drafts_dir.clone();
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

fn draft_path(ledger: &Ledger, id: &str) -> std::path::PathBuf {
    ledger.paths.drafts_dir.join(format!("{id}.json"))
}

fn latest_path(ledger: &Ledger) -> std::path::PathBuf {
    ledger.paths.drafts_dir.join("latest.json")
}

fn read_draft(ledger: &Ledger, id: &str) -> anyhow::Result<CommitDraftV1> {
    let path = draft_path(ledger, id);
    if !path.exists() {
        anyhow::bail!("draft not found: {id}");
    }
    let content = std::fs::read_to_string(&path)?;
    Ok(serde_json::from_str(&content)?)
}

fn write_draft(ledger: &Ledger, draft: &CommitDraftV1) -> anyhow::Result<()> {
    let path = draft_path(ledger, &draft.draft_id);
    std::fs::write(&path, serde_json::to_string_pretty(draft)?)?;
    Ok(())
}

fn write_latest(ledger: &Ledger, draft_id: &str, ts: &str) -> anyhow::Result<()> {
    let latest = serde_json::json!({ "draft_id": draft_id, "ts": ts });
    std::fs::write(
        latest_path(ledger),
        serde_json::to_string_pretty(&latest)?,
    )?;
    Ok(())
}

fn parse_evidence_arg(s: &str) -> anyhow::Result<serde_json::Value> {
    if s.starts_with("evt_") {
        Ok(serde_json::json!({"event_id": s, "why": ""}))
    } else if s.starts_with("blob:sha256:") {
        Ok(serde_json::json!({"blob": s, "why": ""}))
    } else {
        anyhow::bail!("invalid evidence ref: {s} (must start with evt_ or blob:sha256:)")
    }
}

fn key_of_evidence(item: &serde_json::Value) -> Option<String> {
    if let Some(eid) = item.get("event_id").and_then(|x| x.as_str()) {
        return Some(eid.to_string());
    }
    if let Some(blob) = item.get("blob").and_then(|x| x.as_str()) {
        return Some(blob.to_string());
    }
    None
}

fn update_latest_after_delete(ledger: &Ledger, deleted_id: &str) -> anyhow::Result<()> {
    let lp = latest_path(ledger);
    if !lp.exists() {
        return Ok(());
    }
    let content = std::fs::read_to_string(&lp)?;
    if let Ok(val) = serde_json::from_str::<serde_json::Value>(&content) {
        if val.get("draft_id").and_then(|x| x.as_str()) == Some(deleted_id) {
            std::fs::remove_file(&lp)?;
        }
    }
    Ok(())
}

fn sha256_of_file(path: &Path) -> anyhow::Result<String> {
    let bytes = std::fs::read(path)?;
    Ok(hex::encode(Sha256::digest(&bytes)))
}

fn sha256_of_bytes(data: &[u8]) -> String {
    hex::encode(Sha256::digest(data))
}

fn actor_has_role(actors: &ActorsConfig, actor: &str, role: &str) -> bool {
    actors
        .actors
        .get(actor)
        .map(|def| def.roles.contains(&role.to_string()))
        .unwrap_or(false)
}

// ── Public commands ──

pub struct ProposeParams<'a> {
    pub repo_root: &'a Path,
    pub title: &'a str,
    pub purpose: Option<&'a str>,
    pub contrib: Option<&'a str>,
    pub evidence_args: &'a [String],
    pub labels: Vec<String>,
    pub auto: bool,
    pub max_evidence: usize,
}

pub fn propose(p: ProposeParams<'_>) -> anyhow::Result<()> {
    let ledger = Ledger::open(p.repo_root)?;
    let _lock = WorkspaceLock::acquire(&ledger.paths)?;

    let branch = ledger.head_branch()?;
    let base_parent_hash = ledger.last_event_hash()?.unwrap_or_default();

    // Parse manual evidence
    let manual_evidence: Vec<serde_json::Value> = p
        .evidence_args
        .iter()
        .map(|s| parse_evidence_arg(s))
        .collect::<anyhow::Result<Vec<_>>>()?;

    // Auto-evidence
    let should_auto = p.auto || manual_evidence.is_empty();
    let mut evidence = manual_evidence.clone();
    let mut auto_preview: Vec<String> = Vec::new();

    if should_auto {
        let auto_result = build_auto_evidence(&ledger, &branch, p.max_evidence)?;
        let manual_keys: HashSet<String> =
            manual_evidence.iter().filter_map(key_of_evidence).collect();
        for item in auto_result.items {
            if let Some(key) = key_of_evidence(&item) {
                if manual_keys.contains(&key) {
                    continue;
                }
            }
            evidence.push(item);
        }
        auto_preview = auto_result.preview_lines;
    }

    // Policy v2 route selection
    let policy = load_policy_v2(&ledger)?;
    let actors = load_actors(&ledger)?;
    let has_failed_cmd = evidence_has_failed_cmd_check(&ledger, &evidence)?;
    let (rule_id, policy_stages) =
        route_select(&policy, &p.labels, has_failed_cmd, evidence.len());
    let draft_stages = build_draft_stages(&policy_stages, &actors);
    let need_approval = !draft_stages.is_empty();

    // Build preview event (not written to ledger — DRAFT-01)
    let prev_summary = last_commit_contribution(&ledger, &branch)?.unwrap_or_default();
    let contribution = p.contrib.unwrap_or(p.title).to_string();

    let preview_event = new_commit_event(&mut CommitEventParams {
        branch: &branch,
        parent_hash: if base_parent_hash.is_empty() {
            None
        } else {
            Some(&base_parent_hash)
        },
        title: p.title,
        purpose: p.purpose,
        prev_summary: &prev_summary,
        contribution: &contribution,
        evidence: evidence.clone(),
        labels: p.labels.clone(),
    })?;
    let event_preview = serde_json::to_value(&preview_event)?;

    // Build draft
    let draft_id = new_draft_id();
    let created_at = now_rfc3339();

    let draft = CommitDraftV1 {
        version: 1,
        draft_id: draft_id.clone(),
        created_at: created_at.clone(),
        branch: branch.clone(),
        base_parent_hash: base_parent_hash.clone(),
        title: p.title.to_string(),
        purpose: p.purpose.unwrap_or("").to_string(),
        contribution,
        labels: p.labels,
        evidence,
        auto_preview_lines: auto_preview.clone(),
        event_preview,
        status: "proposed".to_string(),
        approvals: vec![],
        applied_commit_id: String::new(),
        policy_require_approval: need_approval,
        policy_min_approvals: if need_approval { 1 } else { 0 },
        stages: draft_stages.clone(),
        route_rule_id: rule_id.clone(),
    };

    // Write draft file
    let dir = ensure_drafts_dir(&ledger)?;
    let path = dir.join(format!("{draft_id}.json"));
    let draft_json = serde_json::to_string_pretty(&draft)?;
    let draft_sha256 = sha256_of_bytes(draft_json.as_bytes());
    std::fs::write(&path, &draft_json)?;

    // Write latest.json
    write_latest(&ledger, &draft_id, &created_at)?;

    // Emit approval_request events for each stage
    if !draft_stages.is_empty() {
        for stage in &draft_stages {
            let parent_hash = ledger.last_event_hash()?;
            let reason = format!("matched rule {rule_id}");
            let req_event = new_approval_request_event(&ApprovalRequestParams {
                branch: &branch,
                parent_hash: parent_hash.as_deref(),
                draft_id: &draft_id,
                draft_sha256: &draft_sha256,
                route_rule_id: &rule_id,
                stage_id: &stage.stage_id,
                role: &stage.role,
                assignees: &stage.assignees,
                reason: &reason,
            })?;
            ledger.append_event(&req_event, true)?;
        }
        rebuild_all(&ledger)?;
    }

    // Print summary
    println!("Draft created: {draft_id}");
    println!("  path: {}", path.display());
    println!("  branch: {branch}");
    println!(
        "  base_parent_hash: {}",
        if base_parent_hash.is_empty() {
            "(none)"
        } else {
            &base_parent_hash
        }
    );
    println!("  policy: require_approval={need_approval} rule={rule_id}");
    if !draft_stages.is_empty() {
        println!("  stages:");
        for s in &draft_stages {
            println!(
                "    - {} (role={}, min_approvals={}, assignees={:?})",
                s.stage_id, s.role, s.min_approvals, s.assignees
            );
        }
    }
    if !auto_preview.is_empty() {
        println!("  auto-evidence ({} items):", auto_preview.len());
        for line in &auto_preview {
            println!("    {line}");
        }
    }

    Ok(())
}

pub fn show(repo_root: &Path, id: &str) -> anyhow::Result<()> {
    let ledger = Ledger::open(repo_root)?;
    let _lock = WorkspaceLock::acquire(&ledger.paths)?;

    let draft = read_draft(&ledger, id)?;
    println!("{}", serde_json::to_string_pretty(&draft)?);
    Ok(())
}

pub fn list(repo_root: &Path, json: bool) -> anyhow::Result<()> {
    let ledger = Ledger::open(repo_root)?;
    let _lock = WorkspaceLock::acquire(&ledger.paths)?;

    let dir = &ledger.paths.drafts_dir;
    if !dir.exists() {
        if !json {
            println!("No drafts.");
        }
        return Ok(());
    }

    let mut drafts: Vec<CommitDraftV1> = Vec::new();

    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let fname = entry.file_name().to_string_lossy().to_string();
        if !fname.ends_with(".json") || fname == "latest.json" {
            continue;
        }
        let content = std::fs::read_to_string(entry.path())?;
        if let Ok(draft) = serde_json::from_str::<CommitDraftV1>(&content) {
            drafts.push(draft);
        }
    }

    if drafts.is_empty() {
        if !json {
            println!("No drafts.");
        }
        return Ok(());
    }

    drafts.sort_by(|a, b| a.created_at.cmp(&b.created_at));

    if json {
        for draft in &drafts {
            let obj = serde_json::json!({
                "draft_id": draft.draft_id,
                "created_at": draft.created_at,
                "branch": draft.branch,
                "title": draft.title,
                "status": draft.status,
                "purpose": draft.purpose,
                "labels": draft.labels,
                "evidence_count": draft.evidence.len(),
                "policy_require_approval": draft.policy_require_approval,
                "route_rule_id": draft.route_rule_id,
                "stages": draft.stages.iter().map(|s| serde_json::json!({
                    "stage_id": s.stage_id,
                    "role": s.role,
                    "status": s.status,
                    "min_approvals": s.min_approvals,
                    "approved_by": s.approved_by,
                    "assignees": s.assignees,
                })).collect::<Vec<_>>(),
                "approvals": draft.approvals.iter().map(|a| serde_json::json!({
                    "actor": a.actor,
                    "decision": a.decision,
                    "ts": a.ts,
                    "stage_id": a.stage_id,
                })).collect::<Vec<_>>(),
            });
            println!("{}", serde_json::to_string(&obj)?);
        }
    } else {
        for draft in &drafts {
            println!(
                "- {} [{}] {} ({}) — {}",
                draft.draft_id, draft.created_at, draft.branch, draft.status, draft.title
            );
        }
    }
    Ok(())
}

pub fn inbox(
    repo_root: &Path,
    by: Option<&str>,
    role: Option<&str>,
    json: bool,
) -> anyhow::Result<()> {
    let ledger = Ledger::open(repo_root)?;
    let _lock = WorkspaceLock::acquire(&ledger.paths)?;

    let dir = &ledger.paths.drafts_dir;
    if !dir.exists() {
        if !json {
            println!("No pending items.");
        }
        return Ok(());
    }

    let mut items: Vec<(String, String, String, String, String, usize, usize, Vec<String>)> =
        Vec::new();

    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let fname = entry.file_name().to_string_lossy().to_string();
        if !fname.ends_with(".json") || fname == "latest.json" {
            continue;
        }
        let content = std::fs::read_to_string(entry.path())?;
        let draft: CommitDraftV1 = match serde_json::from_str(&content) {
            Ok(d) => d,
            Err(_) => continue,
        };
        if draft.status == "applied" {
            continue;
        }
        for stage in &draft.stages {
            if stage.status != "pending" {
                continue;
            }
            if let Some(a) = by {
                if !stage.assignees.contains(&a.to_string()) {
                    continue;
                }
            }
            if let Some(r) = role {
                if stage.role != r {
                    continue;
                }
            }
            items.push((
                draft.draft_id.clone(),
                draft.title.clone(),
                draft.branch.clone(),
                stage.stage_id.clone(),
                stage.role.clone(),
                stage.min_approvals,
                stage.approved_by.len(),
                stage.assignees.clone(),
            ));
        }
    }

    if items.is_empty() {
        if !json {
            println!("No pending items.");
        }
        return Ok(());
    }

    if json {
        for (did, title, branch, sid, role, min_approvals, current, assignees) in &items {
            let obj = serde_json::json!({
                "draft_id": did,
                "title": title,
                "branch": branch,
                "stage_id": sid,
                "role": role,
                "min_approvals": min_approvals,
                "current_approvals": current,
                "approvals_needed": min_approvals.saturating_sub(*current),
                "assignees": assignees,
            });
            println!("{}", serde_json::to_string(&obj)?);
        }
    } else {
        for (did, title, _branch, sid, role, min, current, _assignees) in &items {
            let needed = min.saturating_sub(*current);
            println!("{did} | {title} | {sid} | {role} | approvals needed: {needed}");
        }
    }
    Ok(())
}

pub fn approve(
    repo_root: &Path,
    id: &str,
    actor: &str,
    note: &str,
    stage_id: Option<&str>,
) -> anyhow::Result<()> {
    let ledger = Ledger::open(repo_root)?;
    let _lock = WorkspaceLock::acquire(&ledger.paths)?;

    let mut draft = read_draft(&ledger, id)?;
    if draft.status == "applied" {
        anyhow::bail!("draft already applied: {id}");
    }

    let head = ledger.head_branch()?;
    if head != draft.branch {
        anyhow::bail!(
            "draft branch mismatch: draft={}, head={head}",
            draft.branch
        );
    }

    if !draft.stages.is_empty() {
        // Stage-aware approval
        let sid = stage_id.ok_or_else(|| {
            let pending: Vec<_> = draft
                .stages
                .iter()
                .filter(|s| s.status == "pending")
                .map(|s| format!("{} (role={})", s.stage_id, s.role))
                .collect();
            anyhow::anyhow!(
                "specify --stage <stage_id>. Pending stages: {}",
                pending.join(", ")
            )
        })?;

        let stage_role = {
            let stage = draft
                .stages
                .iter()
                .find(|s| s.stage_id == sid)
                .ok_or_else(|| anyhow::anyhow!("stage not found: {sid}"))?;
            if stage.status != "pending" {
                anyhow::bail!("stage '{sid}' is already {}", stage.status);
            }
            // Validate actor
            let actors = load_actors(&ledger)?;
            let is_assigned = stage.assignees.contains(&actor.to_string());
            let has_role = actor_has_role(&actors, actor, &stage.role);
            if !is_assigned && !has_role && !actors.actors.is_empty() {
                anyhow::bail!(
                    "actor '{actor}' is not assigned to stage '{sid}' and does not have role '{}'",
                    stage.role
                );
            }
            stage.role.clone()
        };

        // Write approval event
        let draft_file = draft_path(&ledger, id);
        let draft_sha256 = sha256_of_file(&draft_file)?;
        let parent_hash = ledger.last_event_hash()?;
        let event = new_approval_event(&ApprovalEventParams {
            branch: &head,
            parent_hash: parent_hash.as_deref(),
            draft_id: id,
            draft_sha256: &draft_sha256,
            decision: "approve",
            actor,
            note,
            stage_id: sid,
            role: &stage_role,
        })?;
        ledger.append_event(&event, true)?;

        // Update stage
        let stage = draft
            .stages
            .iter_mut()
            .find(|s| s.stage_id == sid)
            .unwrap();
        if !stage.approved_by.contains(&actor.to_string()) {
            stage.approved_by.push(actor.to_string());
        }
        if stage.approved_by.len() >= stage.min_approvals {
            stage.status = "approved".to_string();
        }

        // Update draft-level record
        draft.approvals.push(ApprovalRecord {
            ts: now_rfc3339(),
            actor: actor.to_string(),
            decision: "approve".to_string(),
            note: note.to_string(),
            approval_event_id: event.event_id.clone(),
            stage_id: sid.to_string(),
            role: stage_role.clone(),
        });

        if has_stage_reject(&draft) {
            draft.status = "rejected".to_string();
        } else if all_stages_approved(&draft) {
            draft.status = "approved".to_string();
        }

        write_draft(&ledger, &draft)?;
        rebuild_all(&ledger)?;

        let stage_ref = draft.stages.iter().find(|s| s.stage_id == sid).unwrap();
        println!(
            "Approved draft {id} stage {sid} by {actor} (stage: {}, {}/{})",
            stage_ref.status,
            stage_ref.approved_by.len(),
            stage_ref.min_approvals
        );
        println!("  {}", event.event_id);
    } else {
        // Flat v1-style approval
        let draft_file = draft_path(&ledger, id);
        let draft_sha256 = sha256_of_file(&draft_file)?;
        let parent_hash = ledger.last_event_hash()?;
        let event = new_approval_event(&ApprovalEventParams {
            branch: &head,
            parent_hash: parent_hash.as_deref(),
            draft_id: id,
            draft_sha256: &draft_sha256,
            decision: "approve",
            actor,
            note,
            stage_id: "",
            role: "",
        })?;
        ledger.append_event(&event, true)?;

        draft.approvals.push(ApprovalRecord {
            ts: now_rfc3339(),
            actor: actor.to_string(),
            decision: "approve".to_string(),
            note: note.to_string(),
            approval_event_id: event.event_id.clone(),
            stage_id: String::new(),
            role: String::new(),
        });

        if has_reject_flat(&draft) {
            draft.status = "rejected".to_string();
        } else if approved_count_flat(&draft) >= draft.policy_min_approvals.max(1) {
            draft.status = "approved".to_string();
        }

        write_draft(&ledger, &draft)?;
        rebuild_all(&ledger)?;

        println!(
            "Approved draft {id} by {actor} (status: {}, approvals: {}/{})",
            draft.status,
            approved_count_flat(&draft),
            draft.policy_min_approvals
        );
        println!("  {}", event.event_id);
    }
    Ok(())
}

pub fn reject(
    repo_root: &Path,
    id: &str,
    actor: &str,
    note: &str,
    stage_id: Option<&str>,
) -> anyhow::Result<()> {
    let ledger = Ledger::open(repo_root)?;
    let _lock = WorkspaceLock::acquire(&ledger.paths)?;

    let mut draft = read_draft(&ledger, id)?;
    if draft.status == "applied" {
        anyhow::bail!("draft already applied: {id}");
    }

    let head = ledger.head_branch()?;
    if head != draft.branch {
        anyhow::bail!(
            "draft branch mismatch: draft={}, head={head}",
            draft.branch
        );
    }

    if !draft.stages.is_empty() {
        // Stage-aware rejection
        let sid = stage_id.ok_or_else(|| {
            let pending: Vec<_> = draft
                .stages
                .iter()
                .filter(|s| s.status == "pending")
                .map(|s| format!("{} (role={})", s.stage_id, s.role))
                .collect();
            anyhow::anyhow!(
                "specify --stage <stage_id>. Pending stages: {}",
                pending.join(", ")
            )
        })?;

        let stage_role = {
            let stage = draft
                .stages
                .iter()
                .find(|s| s.stage_id == sid)
                .ok_or_else(|| anyhow::anyhow!("stage not found: {sid}"))?;
            if stage.status != "pending" {
                anyhow::bail!("stage '{sid}' is already {}", stage.status);
            }
            let actors = load_actors(&ledger)?;
            let is_assigned = stage.assignees.contains(&actor.to_string());
            let has_role = actor_has_role(&actors, actor, &stage.role);
            if !is_assigned && !has_role && !actors.actors.is_empty() {
                anyhow::bail!(
                    "actor '{actor}' is not assigned to stage '{sid}' and does not have role '{}'",
                    stage.role
                );
            }
            stage.role.clone()
        };

        let draft_file = draft_path(&ledger, id);
        let draft_sha256 = sha256_of_file(&draft_file)?;
        let parent_hash = ledger.last_event_hash()?;
        let event = new_approval_event(&ApprovalEventParams {
            branch: &head,
            parent_hash: parent_hash.as_deref(),
            draft_id: id,
            draft_sha256: &draft_sha256,
            decision: "reject",
            actor,
            note,
            stage_id: sid,
            role: &stage_role,
        })?;
        ledger.append_event(&event, true)?;

        let stage = draft
            .stages
            .iter_mut()
            .find(|s| s.stage_id == sid)
            .unwrap();
        stage.status = "rejected".to_string();

        draft.approvals.push(ApprovalRecord {
            ts: now_rfc3339(),
            actor: actor.to_string(),
            decision: "reject".to_string(),
            note: note.to_string(),
            approval_event_id: event.event_id.clone(),
            stage_id: sid.to_string(),
            role: stage_role,
        });
        draft.status = "rejected".to_string();

        write_draft(&ledger, &draft)?;
        rebuild_all(&ledger)?;

        println!("Rejected draft {id} stage {sid} by {actor}");
        println!("  {}", event.event_id);
    } else {
        // Flat v1-style rejection
        let draft_file = draft_path(&ledger, id);
        let draft_sha256 = sha256_of_file(&draft_file)?;
        let parent_hash = ledger.last_event_hash()?;
        let event = new_approval_event(&ApprovalEventParams {
            branch: &head,
            parent_hash: parent_hash.as_deref(),
            draft_id: id,
            draft_sha256: &draft_sha256,
            decision: "reject",
            actor,
            note,
            stage_id: "",
            role: "",
        })?;
        ledger.append_event(&event, true)?;

        draft.approvals.push(ApprovalRecord {
            ts: now_rfc3339(),
            actor: actor.to_string(),
            decision: "reject".to_string(),
            note: note.to_string(),
            approval_event_id: event.event_id.clone(),
            stage_id: String::new(),
            role: String::new(),
        });
        draft.status = "rejected".to_string();

        write_draft(&ledger, &draft)?;
        rebuild_all(&ledger)?;

        println!("Rejected draft {id} by {actor}");
        println!("  {}", event.event_id);
    }
    Ok(())
}

pub fn apply(
    repo_root: &Path,
    id: &str,
    dry_run: bool,
    delete_after: bool,
) -> anyhow::Result<()> {
    let ledger = Ledger::open(repo_root)?;
    let _lock = WorkspaceLock::acquire(&ledger.paths)?;

    let mut draft = read_draft(&ledger, id)?;

    // Branch mismatch check
    let head = ledger.head_branch()?;
    if head != draft.branch {
        anyhow::bail!(
            "draft branch mismatch: draft={}, head={head}",
            draft.branch
        );
    }

    // Policy gate
    if !draft.stages.is_empty() {
        // Stage-aware gate
        if has_stage_reject(&draft) {
            let rejected: Vec<_> = draft
                .stages
                .iter()
                .filter(|s| s.status == "rejected")
                .map(|s| s.stage_id.as_str())
                .collect();
            anyhow::bail!(
                "draft has rejected stages: [{}]; cannot apply: {id}",
                rejected.join(", ")
            );
        }
        if !all_stages_approved(&draft) {
            let pending: Vec<_> = draft
                .stages
                .iter()
                .filter(|s| s.status == "pending")
                .map(|s| {
                    format!(
                        "{} ({}/{}, assignees: {:?})",
                        s.stage_id,
                        s.approved_by.len(),
                        s.min_approvals,
                        s.assignees
                    )
                })
                .collect();
            anyhow::bail!(
                "policy gate: not all stages approved. Pending: {}",
                pending.join("; ")
            );
        }
    } else {
        // Flat v1 gate
        if has_reject_flat(&draft) {
            anyhow::bail!("draft has reject decision; cannot apply: {id}");
        }
        let pol = load_policy_v2(&ledger)?;
        let has_failed_cmd = evidence_has_failed_cmd_check(&ledger, &draft.evidence)?;
        let (_, policy_stages) =
            route_select(&pol, &draft.labels, has_failed_cmd, draft.evidence.len());
        let need_approval = !policy_stages.is_empty();
        if need_approval {
            let ok = approved_count_flat(&draft);
            let need = draft.policy_min_approvals.max(1);
            if ok < need {
                anyhow::bail!(
                    "policy gate: approvals {ok}/{need} not satisfied. Run: edda draft approve {id} --by <actor>"
                );
            }
        }
    }

    // Rebase (CONTRACT DRAFT-02)
    let new_parent_hash = ledger.last_event_hash()?;
    let prev_summary = last_commit_contribution(&ledger, &head)?.unwrap_or_default();

    let mut labels = draft.labels.clone();
    let new_ph = new_parent_hash.as_deref().unwrap_or("");
    if draft.base_parent_hash != new_ph && !labels.contains(&"draft_rebased".to_string()) {
        labels.push("draft_rebased".to_string());
    }

    let need_approval = !draft.stages.is_empty() || draft.policy_require_approval;
    if need_approval && !labels.contains(&"approved".to_string()) {
        labels.push("approved".to_string());
    }

    let event = new_commit_event(&mut CommitEventParams {
        branch: &head,
        parent_hash: new_parent_hash.as_deref(),
        title: &draft.title,
        purpose: if draft.purpose.is_empty() {
            None
        } else {
            Some(&draft.purpose)
        },
        prev_summary: &prev_summary,
        contribution: &draft.contribution,
        evidence: draft.evidence.clone(),
        labels,
    })?;

    if dry_run {
        println!("{}", serde_json::to_string_pretty(&event)?);
        println!("(dry-run) not written.");
        return Ok(());
    }

    ledger.append_event(&event, true)?;
    rebuild_all(&ledger)?;

    draft.status = "applied".to_string();
    draft.applied_commit_id = event.event_id.clone();
    write_draft(&ledger, &draft)?;

    println!("Applied draft {} -> commit {}", id, event.event_id);

    if delete_after {
        let path = draft_path(&ledger, id);
        if path.exists() {
            std::fs::remove_file(&path)?;
        }
        update_latest_after_delete(&ledger, id)?;
        println!("Deleted draft {id}");
    }

    Ok(())
}

pub fn delete(repo_root: &Path, id: &str) -> anyhow::Result<()> {
    let ledger = Ledger::open(repo_root)?;
    let _lock = WorkspaceLock::acquire(&ledger.paths)?;

    let path = draft_path(&ledger, id);
    if !path.exists() {
        anyhow::bail!("draft not found: {id}");
    }

    std::fs::remove_file(&path)?;
    update_latest_after_delete(&ledger, id)?;

    println!("Deleted draft {id}");
    Ok(())
}
