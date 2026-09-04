#[test]
fn codex_bin_falls_back_to_the_platform_install() {
    // npm ships codex as an extensionless sh launcher plus codex.cmd on
    // Windows, with no codex.exe, and CreateProcess does not apply
    // PATHEXT — the bare name never resolves there.
    let expected = if cfg!(windows) { "codex.cmd" } else { "codex" };
    assert_eq!(resolve_codex_bin(None), PathBuf::from(expected));
}

#[test]
fn edda_codex_bin_overrides_the_platform_default() {
    let custom = "/opt/codex/bin/codex-custom";
    assert_eq!(
        resolve_codex_bin(Some(OsString::from(custom))),
        PathBuf::from(custom)
    );
}

#[test]
fn empty_edda_codex_bin_is_treated_as_unset() {
    let expected = if cfg!(windows) { "codex.cmd" } else { "codex" };
    assert_eq!(
        resolve_codex_bin(Some(OsString::new())),
        PathBuf::from(expected),
        "an empty override must not produce an unspawnable empty path"
    );
}

#[test]
fn with_bin_overrides_the_default() {
    let custom = PathBuf::from("/opt/codex/bin/codex");
    assert_eq!(CodexLauncher::with_bin(custom.clone()).codex_bin, custom);
}
