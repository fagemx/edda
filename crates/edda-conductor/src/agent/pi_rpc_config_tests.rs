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
