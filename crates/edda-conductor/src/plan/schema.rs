use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// A multi-phase AI coding plan.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Plan {
    /// Kebab-case plan name.
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    /// High-level intent — injected into every phase so agents stay aligned.
    #[serde(default)]
    pub purpose: Option<String>,
    #[serde(default)]
    pub cwd: Option<String>,
    #[serde(default)]
    pub budget_usd: Option<f64>,
    #[serde(default = "default_max_attempts")]
    pub max_attempts: u32,
    #[serde(default = "default_timeout_sec")]
    pub timeout_sec: u64,
    #[serde(default)]
    pub on_fail: OnFail,
    #[serde(default)]
    pub env: HashMap<String, String>,
    #[serde(default)]
    pub tags: Vec<String>,
    pub phases: Vec<Phase>,
}

/// A single phase within a plan.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Phase {
    /// Kebab-case phase ID, unique within the plan.
    pub id: String,
    pub prompt: String,
    #[serde(default)]
    pub cwd: Option<String>,
    #[serde(default)]
    pub depends_on: Vec<String>,
    #[serde(default)]
    pub check: Vec<CheckSpec>,
    #[serde(default)]
    pub max_attempts: Option<u32>,
    #[serde(default)]
    pub timeout_sec: Option<u64>,
    #[serde(default)]
    pub on_fail: Option<OnFail>,
    #[serde(default)]
    pub context: Option<String>,
    #[serde(default)]
    pub env: HashMap<String, String>,
    #[serde(default)]
    pub budget_usd: Option<f64>,
    #[serde(default)]
    pub allowed_tools: Option<Vec<String>>,
    #[serde(default = "default_permission_mode")]
    pub permission_mode: String,
}

/// Failure policy for a phase.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum OnFail {
    /// Ralph loop: auto-retry with check failure feedback until max_attempts exhausted,
    /// then fall back to Ask.
    #[default]
    AutoRetry,
    Ask,
    Skip,
    Abort,
}

/// Check specification — what to verify after a phase completes.
///
/// In YAML, checks can be written in short format (`cmd_succeeds: "cargo test"`)
/// or tagged format (`{ type: cmd_succeeds, cmd: "cargo test" }`).
/// Short format is normalized to tagged during parsing.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum CheckSpec {
    FileExists {
        path: String,
    },
    CmdSucceeds {
        cmd: String,
        #[serde(default = "default_cmd_timeout")]
        timeout_sec: u64,
    },
    FileContains {
        path: String,
        pattern: String,
    },
    GitClean {
        #[serde(default)]
        allow_untracked: bool,
    },
    EddaEvent {
        event_type: String,
        #[serde(default)]
        after: Option<String>,
    },
    WaitUntil {
        check: Box<CheckSpec>,
        #[serde(default = "default_wait_interval")]
        interval_sec: u64,
        #[serde(default = "default_wait_timeout")]
        timeout_sec: u64,
        #[serde(default)]
        backoff: BackoffStrategy,
    },
}

impl CheckSpec {
    /// Human-readable type name.
    pub fn type_name(&self) -> &'static str {
        match self {
            CheckSpec::FileExists { .. } => "file_exists",
            CheckSpec::CmdSucceeds { .. } => "cmd_succeeds",
            CheckSpec::FileContains { .. } => "file_contains",
            CheckSpec::GitClean { .. } => "git_clean",
            CheckSpec::EddaEvent { .. } => "edda_event",
            CheckSpec::WaitUntil { .. } => "wait_until",
        }
    }

    /// Whether this check type is retryable on failure.
    pub fn is_retryable(&self) -> bool {
        match self {
            CheckSpec::CmdSucceeds { .. } => true,
            CheckSpec::FileExists { .. } => true,
            CheckSpec::FileContains { .. } => true,
            CheckSpec::GitClean { .. } => true,
            CheckSpec::EddaEvent { .. } => true,
            CheckSpec::WaitUntil { .. } => false, // already has internal retry
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BackoffStrategy {
    None,
    #[default]
    Linear,
    Exponential,
}

fn default_max_attempts() -> u32 {
    3
}
fn default_timeout_sec() -> u64 {
    1800
}
fn default_permission_mode() -> String {
    "bypassPermissions".into()
}
fn default_cmd_timeout() -> u64 {
    120
}
fn default_wait_interval() -> u64 {
    30
}
fn default_wait_timeout() -> u64 {
    600
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn on_fail_default_is_auto_retry() {
        assert_eq!(OnFail::default(), OnFail::AutoRetry);
    }

    #[test]
    fn check_spec_type_names() {
        let c = CheckSpec::CmdSucceeds {
            cmd: "echo ok".into(),
            timeout_sec: 120,
        };
        assert_eq!(c.type_name(), "cmd_succeeds");
        assert!(c.is_retryable());

        let w = CheckSpec::WaitUntil {
            check: Box::new(c),
            interval_sec: 30,
            timeout_sec: 600,
            backoff: BackoffStrategy::Linear,
        };
        assert_eq!(w.type_name(), "wait_until");
        assert!(!w.is_retryable());
    }

    #[test]
    fn plan_deserialize_minimal() {
        let yaml = r#"
name: test-plan
phases:
  - id: step-one
    prompt: "Do something"
"#;
        let plan: Plan = serde_yml::from_str(yaml).unwrap();
        assert_eq!(plan.name, "test-plan");
        assert_eq!(plan.phases.len(), 1);
        assert_eq!(plan.max_attempts, 3);
        assert_eq!(plan.on_fail, OnFail::AutoRetry);
        assert_eq!(plan.phases[0].permission_mode, "bypassPermissions");
        assert!(plan.purpose.is_none());
    }

    #[test]
    fn plan_deserialize_with_purpose() {
        let yaml = r#"
name: todo-app
purpose: "Simple todo app, keep it minimal"
phases:
  - id: db
    prompt: "Build schema"
"#;
        let plan: Plan = serde_yml::from_str(yaml).unwrap();
        assert_eq!(
            plan.purpose.as_deref(),
            Some("Simple todo app, keep it minimal")
        );
    }

    #[test]
    fn phase_deserialize_full() {
        let yaml = r#"
name: full
phases:
  - id: build
    prompt: "Build it"
    depends_on: []
    max_attempts: 5
    timeout_sec: 600
    on_fail: abort
    context: "Phase 1"
    env:
      FOO: bar
    allowed_tools: [Read, Write]
    permission_mode: default
    check:
      - type: cmd_succeeds
        cmd: "cargo build"
      - type: file_exists
        path: "target/debug/main"
"#;
        let plan: Plan = serde_yml::from_str(yaml).unwrap();
        let phase = &plan.phases[0];
        assert_eq!(phase.max_attempts, Some(5));
        assert_eq!(phase.on_fail, Some(OnFail::Abort));
        assert_eq!(phase.check.len(), 2);
        assert_eq!(phase.env.get("FOO").unwrap(), "bar");
    }
}
