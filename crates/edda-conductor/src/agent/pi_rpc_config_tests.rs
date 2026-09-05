#[test]
fn pi_bin_falls_back_to_the_platform_install() {
    // npm ships pi as a .cmd shim on Windows with no .exe, and
    // CreateProcess does not apply PATHEXT — the bare name never resolves.
    let expected = if cfg!(windows) { "pi.cmd" } else { "pi" };
    assert_eq!(resolve_pi_bin(None), PathBuf::from(expected));
}

#[test]
fn edda_pi_bin_overrides_the_platform_default() {
    let custom = "/opt/pi/bin/pi-custom";
    assert_eq!(
        resolve_pi_bin(Some(OsString::from(custom))),
        PathBuf::from(custom)
    );
}

#[test]
fn empty_edda_pi_bin_is_treated_as_unset() {
    let expected = if cfg!(windows) { "pi.cmd" } else { "pi" };
    assert_eq!(
        resolve_pi_bin(Some(OsString::new())),
        PathBuf::from(expected),
        "an empty override must not produce an unspawnable empty path"
    );
}

#[test]
fn with_bin_overrides_the_default() {
    let custom = PathBuf::from("/opt/pi/bin/pi");
    assert_eq!(PiRpcLauncher::with_bin(custom.clone()).pi_bin, custom);
}

// ── GH-574: phase-declared capabilities reach the pi spawn line ──

fn pi_args_for(launcher: &PiRpcLauncher, yaml: &str) -> Vec<String> {
    let phase = phase_from_yaml(yaml);
    let cmd = launcher.build_command(&phase, "sess-1", Path::new("."));
    cmd.as_std()
        .get_args()
        .map(|a| a.to_string_lossy().into_owned())
        .collect()
}

#[test]
fn pi_phase_capabilities_reach_the_spawn_line() {
    let launcher = PiRpcLauncher::new();
    let args = pi_args_for(
        &launcher,
        "  - id: a\n    prompt: x\n    model: anthropic/claude-opus-5\n    thinking: high\n    tools: [read, grep]\n    exclude_tools: [edit, write]\n",
    );
    for (flag, value) in [
        ("--model", "anthropic/claude-opus-5"),
        ("--thinking", "high"),
        ("--tools", "read,grep"),
        ("--exclude-tools", "edit,write"),
    ] {
        let pos = args
            .iter()
            .position(|a| a == flag)
            .unwrap_or_else(|| panic!("{flag} must appear in the pi spawn line: {args:?}"));
        assert_eq!(args[pos + 1], value, "value after {flag}");
    }
}

#[test]
fn pi_phase_model_wins_over_the_builder_fallback() {
    let launcher = PiRpcLauncher::new().with_model("openai-codex/gpt-5.6-sol");
    let args = pi_args_for(&launcher, "  - id: a\n    prompt: x\n");
    let pos = args.iter().position(|a| a == "--model").expect("--model");
    assert_eq!(args[pos + 1], "openai-codex/gpt-5.6-sol");

    let args = pi_args_for(
        &launcher,
        "  - id: a\n    prompt: x\n    model: anthropic/claude-opus-5\n",
    );
    let pos = args.iter().position(|a| a == "--model").expect("--model");
    assert_eq!(args[pos + 1], "anthropic/claude-opus-5");
}

#[test]
fn pi_no_declarations_spawn_no_capability_flags() {
    let args = pi_args_for(&PiRpcLauncher::new(), "  - id: a\n    prompt: x\n");
    for flag in ["--model", "--thinking", "--tools", "--exclude-tools"] {
        assert!(
            !args.contains(&flag.to_string()),
            "{flag} must be absent without a declaration: {args:?}"
        );
    }
}

#[test]
fn pi_refuses_thinking_flag_plus_model_suffix() {
    let launcher = PiRpcLauncher::new();
    let phase = phase_from_yaml(
        "  - id: a\n    prompt: x\n    model: openai-codex/gpt-5.6-sol:high\n    thinking: low\n",
    );
    let error = launcher
        .validate_phase(&phase)
        .expect_err("the ambiguous combination must be refused");
    assert!(error.to_string().contains("refuses to guess"), "{error}");
}

#[test]
fn pi_accepts_thinking_flag_without_model_suffix() {
    let launcher = PiRpcLauncher::new();
    let phase = phase_from_yaml(
        "  - id: a\n    prompt: x\n    model: openai-codex/gpt-5.6-sol\n    thinking: low\n",
    );
    launcher
        .validate_phase(&phase)
        .expect("a plain provider/id pattern with --thinking is unambiguous");
}

#[test]
fn list_models_fails_loudly_when_pi_is_missing() {
    let error = list_models(Some(PathBuf::from("definitely-not-pi-xyz-8f3a")), None)
        .expect_err("a missing pi binary must be an explicit error");
    assert!(error.to_string().contains("list-models"), "{error}");
}
