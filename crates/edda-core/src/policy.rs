//! Governance policy types and RBAC evaluation.
//!
//! Shared between `edda-cli` (draft approval workflow) and `edda-serve` (authz API).

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;

// ── Policy v2 data model ──

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct PolicyV2Config {
    pub version: u32,
    #[serde(default)]
    pub roles: Vec<String>,
    #[serde(default)]
    pub rules: Vec<PolicyRule>,
    /// RBAC permissions (optional, additive to v2 schema).
    #[serde(default)]
    pub permissions: Option<PermissionsConfig>,
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
    /// Actor kind: "user" or "agent". Defaults to "user" for v1 compat.
    #[serde(default = "default_user_kind")]
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    /// For agent actors: runtime platform (e.g. "claude", "opencode").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime: Option<String>,
}

fn default_user_kind() -> String {
    "user".into()
}

impl Default for ActorsConfig {
    fn default() -> Self {
        Self {
            version: 1,
            actors: BTreeMap::new(),
        }
    }
}

// ── RBAC Permissions ──

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PermissionsConfig {
    /// "deny" or "allow" — what happens when no grant matches.
    #[serde(default = "default_deny")]
    pub default: String,
    #[serde(default)]
    pub grants: Vec<PermissionGrant>,
}

fn default_deny() -> String {
    "deny".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PermissionGrant {
    pub actions: Vec<String>,
    pub roles: Vec<String>,
}

// ── Authz request / result ──

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthzRequest {
    pub actor: String,
    pub action: String,
    #[serde(default)]
    pub resource: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthzResult {
    pub allowed: bool,
    pub actor_roles: Vec<String>,
    pub matched_grant: Option<PermissionGrant>,
    pub policy_default: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

// ── Evaluation ──

/// Evaluate whether an actor is allowed to perform an action.
///
/// Logic:
/// 1. Look up actor's roles from `actors`.
/// 2. Find a matching grant in `policy.permissions.grants` where the action
///    matches AND at least one of the actor's roles (or `"*"`) is listed.
/// 3. If no grant matches, fall back to `permissions.default`.
pub fn evaluate_authz(
    req: &AuthzRequest,
    policy: &PolicyV2Config,
    actors: &ActorsConfig,
) -> AuthzResult {
    let actor_roles: Vec<String> = actors
        .actors
        .get(&req.actor)
        .map(|a| a.roles.clone())
        .unwrap_or_default();

    let permissions = match &policy.permissions {
        Some(p) => p,
        None => {
            // No permissions section → use default deny
            return AuthzResult {
                allowed: false,
                actor_roles,
                matched_grant: None,
                policy_default: "deny".to_string(),
                reason: Some("no permissions section in policy".to_string()),
            };
        }
    };

    let policy_default = &permissions.default;

    // Search for a matching grant
    for grant in &permissions.grants {
        if !grant.actions.contains(&req.action) {
            continue;
        }
        // Check if any of actor's roles match the grant's roles
        let role_match = grant
            .roles
            .iter()
            .any(|r| r == "*" || actor_roles.iter().any(|ar| ar == r));
        if role_match {
            return AuthzResult {
                allowed: true,
                actor_roles,
                matched_grant: Some(grant.clone()),
                policy_default: policy_default.clone(),
                reason: None,
            };
        }
    }

    // No grant matched — apply default
    let allowed = policy_default == "allow";
    let reason = if allowed {
        None
    } else {
        Some(format!(
            "no grant matches action '{}' for roles {:?}",
            req.action, actor_roles
        ))
    };

    AuthzResult {
        allowed,
        actor_roles,
        matched_grant: None,
        policy_default: policy_default.clone(),
        reason,
    }
}

// ── File loading helpers ──

/// Load policy.yaml from a directory containing `.edda/`.
pub fn load_policy_from_dir(edda_dir: &Path) -> anyhow::Result<PolicyV2Config> {
    let path = edda_dir.join("policy.yaml");
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
            permissions: None,
        });
    }
    let content = std::fs::read(&path)?;
    let v2: PolicyV2Config = serde_yaml::from_slice(&content)?;
    Ok(v2)
}

/// Load actors.yaml from a directory containing `.edda/`.
pub fn load_actors_from_dir(edda_dir: &Path) -> anyhow::Result<ActorsConfig> {
    let path = edda_dir.join("actors.yaml");
    if !path.exists() {
        return Ok(ActorsConfig::default());
    }
    let content = std::fs::read(&path)?;
    let cfg: ActorsConfig = serde_yaml::from_slice(&content)?;
    Ok(cfg)
}

/// Save actors.yaml to the `.edda/` directory.
pub fn save_actors_to_dir(edda_dir: &Path, cfg: &ActorsConfig) -> anyhow::Result<()> {
    let path = edda_dir.join("actors.yaml");
    let yaml = serde_yaml::to_string(cfg)?;
    std::fs::write(&path, yaml.as_bytes())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_policy(default: &str) -> PolicyV2Config {
        PolicyV2Config {
            version: 2,
            roles: vec!["lead".into(), "reviewer".into(), "operator".into()],
            rules: vec![],
            permissions: Some(PermissionsConfig {
                default: default.to_string(),
                grants: vec![
                    PermissionGrant {
                        actions: vec!["deploy".into(), "rollback".into()],
                        roles: vec!["lead".into(), "operator".into()],
                    },
                    PermissionGrant {
                        actions: vec!["merge".into(), "approve".into()],
                        roles: vec!["lead".into(), "reviewer".into()],
                    },
                    PermissionGrant {
                        actions: vec!["read".into()],
                        roles: vec!["*".into()],
                    },
                ],
            }),
        }
    }

    fn actors_with(name: &str, roles: &[&str]) -> ActorsConfig {
        let mut actors = BTreeMap::new();
        actors.insert(
            name.to_string(),
            ActorDef {
                roles: roles.iter().map(|s| s.to_string()).collect(),
                kind: "user".into(),
                email: None,
                display_name: None,
                runtime: None,
            },
        );
        ActorsConfig { version: 1, actors }
    }

    #[test]
    fn test_evaluate_authz_allow() {
        let policy = sample_policy("deny");
        let actors = actors_with("alice", &["operator"]);
        let req = AuthzRequest {
            actor: "alice".into(),
            action: "deploy".into(),
            resource: None,
        };
        let result = evaluate_authz(&req, &policy, &actors);
        assert!(result.allowed);
        assert!(result.matched_grant.is_some());
        assert_eq!(result.actor_roles, vec!["operator"]);
    }

    #[test]
    fn test_evaluate_authz_deny_no_role() {
        let policy = sample_policy("deny");
        let actors = actors_with("bob", &["reviewer"]);
        let req = AuthzRequest {
            actor: "bob".into(),
            action: "deploy".into(),
            resource: None,
        };
        let result = evaluate_authz(&req, &policy, &actors);
        assert!(!result.allowed);
        assert!(result.matched_grant.is_none());
        assert!(result.reason.is_some());
    }

    #[test]
    fn test_evaluate_authz_default_deny() {
        let policy = sample_policy("deny");
        let actors = actors_with("alice", &["operator"]);
        let req = AuthzRequest {
            actor: "alice".into(),
            action: "unknown_action".into(),
            resource: None,
        };
        let result = evaluate_authz(&req, &policy, &actors);
        assert!(!result.allowed);
        assert_eq!(result.policy_default, "deny");
    }

    #[test]
    fn test_evaluate_authz_default_allow() {
        let policy = sample_policy("allow");
        let actors = actors_with("alice", &["operator"]);
        let req = AuthzRequest {
            actor: "alice".into(),
            action: "unknown_action".into(),
            resource: None,
        };
        let result = evaluate_authz(&req, &policy, &actors);
        assert!(result.allowed);
        assert_eq!(result.policy_default, "allow");
    }

    #[test]
    fn test_evaluate_authz_wildcard_role() {
        let policy = sample_policy("deny");
        let actors = actors_with("charlie", &["intern"]);
        let req = AuthzRequest {
            actor: "charlie".into(),
            action: "read".into(),
            resource: None,
        };
        let result = evaluate_authz(&req, &policy, &actors);
        assert!(result.allowed, "wildcard '*' should match any role");
    }

    #[test]
    fn test_evaluate_authz_unknown_actor() {
        let policy = sample_policy("deny");
        let actors = ActorsConfig::default();
        let req = AuthzRequest {
            actor: "nobody".into(),
            action: "deploy".into(),
            resource: None,
        };
        let result = evaluate_authz(&req, &policy, &actors);
        assert!(!result.allowed);
        assert!(result.actor_roles.is_empty());
    }

    #[test]
    fn test_evaluate_authz_unknown_actor_wildcard_grant() {
        let policy = sample_policy("deny");
        // Actor not in actors.yaml but action has wildcard role
        let actors = ActorsConfig::default();
        let req = AuthzRequest {
            actor: "nobody".into(),
            action: "read".into(),
            resource: None,
        };
        let result = evaluate_authz(&req, &policy, &actors);
        // Wildcard matches even with empty roles
        assert!(result.allowed);
    }

    #[test]
    fn test_policy_v2_backward_compat() {
        let yaml = r#"
version: 2
roles:
  - lead
rules:
  - id: default
    when:
      default: true
    stages: []
"#;
        let config: PolicyV2Config = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(config.version, 2);
        assert!(config.permissions.is_none());
    }

    #[test]
    fn test_policy_v2_with_permissions() {
        let yaml = r#"
version: 2
roles:
  - lead
  - operator
rules: []
permissions:
  default: deny
  grants:
    - actions: [deploy]
      roles: [lead, operator]
    - actions: [read]
      roles: ["*"]
"#;
        let config: PolicyV2Config = serde_yaml::from_str(yaml).unwrap();
        assert!(config.permissions.is_some());
        let perms = config.permissions.unwrap();
        assert_eq!(perms.default, "deny");
        assert_eq!(perms.grants.len(), 2);
        assert_eq!(perms.grants[0].actions, vec!["deploy"]);
    }

    #[test]
    fn test_actors_v1_backward_compat() {
        let yaml = r#"
version: 1
actors:
  alice:
    roles: [lead, reviewer]
"#;
        let cfg: ActorsConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(cfg.version, 1);
        let alice = cfg.actors.get("alice").unwrap();
        assert_eq!(alice.roles, vec!["lead", "reviewer"]);
        assert_eq!(alice.kind, "user"); // default
        assert!(alice.email.is_none());
        assert!(alice.display_name.is_none());
        assert!(alice.runtime.is_none());
    }

    #[test]
    fn test_actors_v2_full_fields() {
        let yaml = r#"
version: 2
actors:
  alice:
    kind: user
    roles: [lead, reviewer]
    email: alice@example.com
    display_name: Alice Chen
  claude-agent-1:
    kind: agent
    roles: [operator]
    runtime: claude
"#;
        let cfg: ActorsConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(cfg.version, 2);

        let alice = cfg.actors.get("alice").unwrap();
        assert_eq!(alice.kind, "user");
        assert_eq!(alice.email.as_deref(), Some("alice@example.com"));
        assert_eq!(alice.display_name.as_deref(), Some("Alice Chen"));
        assert!(alice.runtime.is_none());

        let agent = cfg.actors.get("claude-agent-1").unwrap();
        assert_eq!(agent.kind, "agent");
        assert_eq!(agent.roles, vec!["operator"]);
        assert_eq!(agent.runtime.as_deref(), Some("claude"));
        assert!(agent.email.is_none());
    }

    #[test]
    fn test_no_permissions_section_defaults_deny() {
        let policy = PolicyV2Config {
            version: 2,
            roles: vec![],
            rules: vec![],
            permissions: None,
        };
        let actors = actors_with("alice", &["lead"]);
        let req = AuthzRequest {
            actor: "alice".into(),
            action: "deploy".into(),
            resource: None,
        };
        let result = evaluate_authz(&req, &policy, &actors);
        assert!(!result.allowed);
        assert!(result
            .reason
            .as_ref()
            .unwrap()
            .contains("no permissions section"));
    }
}
