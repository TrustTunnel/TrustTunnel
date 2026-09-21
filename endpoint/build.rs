// Build script that resolves the endpoint version the same way the rest of the
// organization does: from a CI-provided override first, then from the git tag.
// The static `version` field in Cargo.toml is a placeholder (0.0.0) and is never
// the source of truth, so the released binary reports the same version as its
// git tag / GitHub release.
//
// Resolution order (mirrors the C++ projects' cmake/version.cmake):
//   1. TRUSTTUNNEL_VERSION environment variable (CI sets it from CHANGELOG.md);
//   2. git describe --tags --match 'v*' (a plain local build is self-versioning);
//   3. 0.0.0-git fallback (never hard-fails the build).
use std::process::Command;

fn resolve_version() -> String {
    if let Ok(v) = std::env::var("TRUSTTUNNEL_VERSION") {
        let v = v.trim();
        if !v.is_empty() {
            return v.to_string();
        }
    }

    if let Ok(out) = Command::new("git")
        .args(["describe", "--tags", "--match", "v*"])
        .output()
    {
        if out.status.success() {
            let described = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if !described.is_empty() {
                return described
                    .strip_prefix('v')
                    .unwrap_or(&described)
                    .to_string();
            }
        }
    }

    "0.0.0-git".to_string()
}

fn main() {
    // Re-run when the override or the resolved git state changes.
    println!("cargo:rerun-if-env-changed=TRUSTTUNNEL_VERSION");
    println!("cargo:rerun-if-changed=../.git/HEAD");

    let version = resolve_version();
    println!("cargo:rustc-env=TRUSTTUNNEL_VERSION={version}");
}
