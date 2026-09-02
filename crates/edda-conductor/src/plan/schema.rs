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
    /// Tool allowlist, passed to the backend verbatim (pi `--tools`, claude
    /// `--allowedTools`). The historical YAML spelling `allowed_tools` still
    /// parses (GH-574 compatibility); both spellings in one phase is a
    /// deserialization error, not a merge.
    #[serde(
        default,
        alias = "allowed_tools",
        skip_serializing_if = "Option::is_none"
    )]
    pub tools: Option<Vec<String>>,
    /// Tool denylist (pi `--exclude-tools`, claude `--disallowedTools`).
    /// Structural enforcement, not prompt discipline: a denied tool never
    /// reaches the agent, regardless of what the prompt says.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exclude_tools: Option<Vec<String>>,
    /// Model selection passed to the backend verbatim (pi `--model
    /// <pattern>`, claude `--model`). Per-backend support varies;
    /// unsupported combinations must be rejected with an explicit error,
    /// never silently ignored (GH-574).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Thinking level (pi `--thinking`: off, minimal, low, medium, high,
    /// xhigh, max). Only pi supports it today; other backends must reject a
    /// phase that declares it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking: Option<String>,
    #[serde(default = "default_permission_mode")]
    pub permission_mode: String,
    /// Verdict gate: after checks pass, the phase pauses in AWAITING_VERDICT
    /// until an external `edda verdict` arrives (D2).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gate: Option<GateKind>,
    /// Gate wait timeout in seconds; None = wait until cancelled.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gate_timeout_sec: Option<u64>,
    /// Policy when the gate verdict rejects. Default: redispatch.
    #[serde(default)]
    pub on_reject: OnReject,
    /// Path globs this phase owns as its write surface. Published in the
    /// phase's auto-claim event so peer lanes can see the scope (GH-561).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub owns: Vec<String>,
}

/// Kind of approval gate a phase can declare (D2).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum GateKind {
    /// External verdict via `edda verdict approve|reject`.
    Verdict,
}

/// What happens when a verdict gate is rejected (D2/D3).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum OnReject {
    /// Run ONE more agent turn in the SAME session with the rejection
    /// comment as prompt, then re-check and re-gate.
    #[default]
    Redispatch,
    /// Fail the phase with the rejection comment as the error.
    Halt,
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
    // 1800s — the same ceiling as a phase's agent turn (default_timeout_sec).
    // The old 120s default made the most natural check — running this
    // workspace's own test suite — structurally unpassable: `cargo test -p
    // edda` measures 60-150s warm (GH-529 live run) and far more cold or
    // with --workspace, so every run timed out and then burned the whole
    // retry ladder. A check verifies what the agent just spent up to 30
    // minutes producing; it must never time out before the work it
    // verifies can even finish.
    1800
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

    // ── Model / thinking / tool policy (GH-574) ──────────────────────

    #[test]
    fn phase_allowed_tools_yaml_still_parses_as_tools() {
        // Compat: every existing plan YAML spells the allowlist
        // `allowed_tools`; it must keep parsing after the rename to `tools`.
        let yaml = r#"
name: compat
phases:
  - id: a
    prompt: "x"
    allowed_tools: [Read, Grep]
"#;
        let plan: Plan = serde_yml::from_str(yaml).unwrap();
        assert_eq!(
            plan.phases[0].tools,
            Some(vec!["Read".into(), "Grep".into()])
        );
    }

    #[test]
    fn phase_tools_is_the_new_canonical_spelling() {
        let yaml = r#"
name: canonical
phases:
  - id: a
    prompt: "x"
    tools: [read, grep]
"#;
        let plan: Plan = serde_yml::from_str(yaml).unwrap();
        assert_eq!(
            plan.phases[0].tools,
            Some(vec!["read".into(), "grep".into()])
        );
    }

    #[test]
    fn phase_both_tool_spellings_fail_loudly() {
        // The two spellings are one field: an alias pair, not a merge.
        let yaml = r#"
name: ambiguous
phases:
  - id: a
    prompt: "x"
    allowed_tools: [Read]
    tools: [Write]
"#;
        assert!(serde_yml::from_str::<Plan>(yaml).is_err());
    }

    #[test]
    fn phase_model_thinking_and_exclude_tools_parse() {
        let yaml = r#"
name: reviewer
phases:
  - id: review
    prompt: "x"
    model: anthropic/claude-opus-5
    thinking: high
    exclude_tools: [edit, write]
"#;
        let plan: Plan = serde_yml::from_str(yaml).unwrap();
        let phase = &plan.phases[0];
        assert_eq!(phase.model.as_deref(), Some("anthropic/claude-opus-5"));
        assert_eq!(phase.thinking.as_deref(), Some("high"));
        assert_eq!(
            phase.exclude_tools,
            Some(vec!["edit".into(), "write".into()])
        );
        assert!(phase.tools.is_none());
    }

    #[test]
    fn phase_model_fields_default_to_none_and_skip_serialization() {
        let yaml = r#"
name: minimal
phases:
  - id: a
    prompt: "x"
"#;
        let plan: Plan = serde_yml::from_str(yaml).unwrap();
        let phase = &plan.phases[0];
        assert!(phase.model.is_none());
        assert!(phase.thinking.is_none());
        assert!(phase.tools.is_none());
        assert!(phase.exclude_tools.is_none());
        let json = serde_json::to_string(phase).unwrap();
        for absent in ["model", "thinking", "tools", "exclude_tools"] {
            assert!(
                !json.contains(absent),
                "unset {absent} must not serialize: {json}"
            );
        }
    }

    // ── Gate fields (D2) ─────────────────────────────────────────────

    #[test]
    fn phase_defaults_have_no_gate() {
        let yaml = r#"
name: test
phases:
  - id: a
    prompt: "x"
"#;
        let plan: Plan = serde_yml::from_str(yaml).unwrap();
        let phase = &plan.phases[0];
        assert!(phase.gate.is_none());
        assert!(phase.gate_timeout_sec.is_none());
        assert_eq!(phase.on_reject, OnReject::Redispatch);
    }

    #[test]
    fn phase_deserialize_gate_verdict() {
        let yaml = r#"
name: test
phases:
  - id: a
    prompt: "x"
    gate: verdict
    gate_timeout_sec: 3600
    on_reject: halt
"#;
        let plan: Plan = serde_yml::from_str(yaml).unwrap();
        let phase = &plan.phases[0];
        assert_eq!(phase.gate, Some(GateKind::Verdict));
        assert_eq!(phase.gate_timeout_sec, Some(3600));
        assert_eq!(phase.on_reject, OnReject::Halt);
    }

    #[test]
    fn phase_unknown_gate_value_fails_loudly() {
        let yaml = r#"
name: test
phases:
  - id: a
    prompt: "x"
    gate: magic_gate
"#;
        let err = serde_yml::from_str::<Plan>(yaml).unwrap_err();
        assert!(err.to_string().to_lowercase().contains("magic_gate"));
    }

    #[test]
    fn phase_unknown_on_reject_value_fails_loudly() {
        let yaml = r#"
name: test
phases:
  - id: a
    prompt: "x"
    gate: verdict
    on_reject: explode
"#;
        assert!(serde_yml::from_str::<Plan>(yaml).is_err());
    }

    // ── Owned write surfaces (GH-561) ─────────────────────────────

    #[test]
    fn phase_owns_parses_and_defaults_empty() {
        let yaml = r#"
name: owns-plan
phases:
  - id: touch-agent
    prompt: "Edit the agent surface"
    owns:
      - "crates/edda-conductor/src/agent/*"
      - "crates/edda-conductor/src/plan/**"
  - id: no-owns
    prompt: "No declared surface"
"#;
        let plan: Plan = serde_yml::from_str(yaml).unwrap();
        assert_eq!(
            plan.phases[0].owns,
            vec![
                "crates/edda-conductor/src/agent/*".to_string(),
                "crates/edda-conductor/src/plan/**".to_string(),
            ]
        );
        assert!(plan.phases[1].owns.is_empty());
    }

    #[test]
    fn phase_without_owns_omits_field_when_serialized() {
        let yaml = r#"
name: minimal
phases:
  - id: a
    prompt: "x"
"#;
        let plan: Plan = serde_yml::from_str(yaml).unwrap();
        let json = serde_json::to_string(&plan.phases[0]).unwrap();
        assert!(
            !json.contains("owns"),
            "empty owns must not serialize: {json}"
        );
    }

    #[test]
    fn gate_kind_serializes_snake_case() {
        let json = serde_json::to_string(&GateKind::Verdict).unwrap();
        assert_eq!(json, r#""verdict""#);
        let back: GateKind = serde_json::from_str(&json).unwrap();
        assert_eq!(back, GateKind::Verdict);
    }
}
