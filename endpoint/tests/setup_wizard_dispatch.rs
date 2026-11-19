use std::process::Command;

fn endpoint() -> Command {
    Command::new(env!("CARGO_BIN_EXE_trusttunnel_endpoint"))
}

#[test]
fn setup_wizard_help_is_dispatched() {
    let output = endpoint()
        .args(["--setup-wizard", "--help"])
        .output()
        .expect("run endpoint");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Usage: trusttunnel_endpoint --setup-wizard"));
}

#[test]
fn endpoint_help_does_not_require_settings() {
    let output = endpoint().arg("--help").output().expect("run endpoint");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Usage: trusttunnel_endpoint"));
    assert!(stdout.contains("--setup-wizard"));
}

#[test]
fn setup_wizard_noninteractive_generates_configs() {
    let temp = std::env::temp_dir().join(format!(
        "trusttunnel-endpoint-setup-wizard-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system time")
            .as_nanos()
    ));
    std::fs::create_dir_all(&temp).expect("create temp dir");

    let output = endpoint()
        .current_dir(&temp)
        .args([
            "--setup-wizard",
            "-m",
            "non-interactive",
            "-a",
            "127.0.0.1:443",
            "-c",
            "user:password",
            "-n",
            "vpn.example.com",
            "--lib-settings",
            "vpn.toml",
            "--hosts-settings",
            "hosts.toml",
            "--cert-type",
            "self-signed",
        ])
        .output()
        .expect("run endpoint");

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    for path in [
        "vpn.toml",
        "hosts.toml",
        "credentials.toml",
        "rules.toml",
        "certs/cert.pem",
        "certs/key.pem",
    ] {
        assert!(temp.join(path).exists(), "missing {path}");
    }

    std::fs::remove_dir_all(&temp).ok();
}

#[cfg(unix)]
#[test]
fn non_utf8_argument_is_not_a_panic() {
    use std::os::unix::ffi::OsStrExt;

    let output = endpoint()
        .arg(std::ffi::OsStr::from_bytes(b"\xff"))
        .output()
        .expect("run endpoint");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!stderr.contains("panicked"), "stderr: {stderr}");
}
