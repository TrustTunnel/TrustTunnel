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
