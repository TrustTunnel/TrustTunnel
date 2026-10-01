use std::process::Command;
use trusttunnel_deeplink::{decode, encode, DeepLinkConfig, Protocol};

/// Get the workspace root directory
fn workspace_root() -> std::path::PathBuf {
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR not set");
    std::path::Path::new(&manifest_dir)
        .parent()
        .expect("Failed to get parent directory")
        .to_path_buf()
}

/// Run the Python config_to_deeplink.py script
fn python_encode(toml_config: &str) -> String {
    use rand::RngExt;

    let mut rng = rand::rng();
    let workspace = workspace_root();
    let temp_file = workspace.join(format!("test_config_{}.toml", rng.random::<u32>()));

    std::fs::write(&temp_file, toml_config).expect("Failed to write temp TOML file");

    let script_path = workspace.join("scripts/config_to_deeplink.py");

    let output = Command::new("python3")
        .arg(&script_path)
        .arg(&temp_file)
        .current_dir(&workspace)
        .output()
        .expect("Failed to run Python script");

    std::fs::remove_file(&temp_file).ok();

    if !output.status.success() {
        panic!(
            "Python script failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    String::from_utf8(output.stdout)
        .expect("Invalid UTF-8 from Python script")
        .trim()
        .to_string()
}

/// Run the Python deeplink_to_config.py script
fn python_decode(uri: &str) -> String {
    let workspace = workspace_root();
    let script_path = workspace.join("scripts/deeplink_to_config.py");

    let output = Command::new("python3")
        .arg(&script_path)
        .arg(uri)
        .current_dir(&workspace)
        .output()
        .expect("Failed to run Python script");

    if !output.status.success() {
        panic!(
            "Python script failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    String::from_utf8(output.stdout).expect("Invalid UTF-8 from Python script")
}

#[test]
fn test_minimal_config_matches_python() {
    let toml = r#"
hostname = "vpn.example.com"
addresses = ["1.2.3.4:443"]
username = "alice"
password = "secret123"
"#;

    // Rust encode
    let config = DeepLinkConfig::builder()
        .hostname("vpn.example.com".to_string())
        .addresses(vec!["1.2.3.4:443".to_string()])
        .username("alice".to_string())
        .password("secret123".to_string())
        .build()
        .unwrap();

    let rust_uri = encode(&config).unwrap();
    let python_uri = python_encode(toml);

    assert_eq!(
        rust_uri, python_uri,
        "Rust and Python encoders produced different URIs"
    );

    // Verify roundtrip through Python decoder
    let python_decoded = python_decode(&rust_uri);
    assert!(
        python_decoded.contains("hostname = \"vpn.example.com\""),
        "Python decoder failed to decode Rust-encoded URI"
    );
    assert!(
        python_decoded.contains("username = \"alice\""),
        "Python decoder failed to decode username"
    );
}

#[test]
fn test_full_config_matches_python() {
    let toml = r#"
hostname = "secure.vpn.example.com"
addresses = ["192.168.1.1:8443", "10.0.0.1:443"]
username = "premium_user"
password = "very_secret_password"
custom_sni = "cdn.example.org"
has_ipv6 = false
upstream_protocol = "http3"
anti_dpi = true
skip_verification = false
"#;

    // Rust encode
    let config = DeepLinkConfig::builder()
        .hostname("secure.vpn.example.com".to_string())
        .addresses(vec![
            "192.168.1.1:8443".to_string(),
            "10.0.0.1:443".to_string(),
        ])
        .username("premium_user".to_string())
        .password("very_secret_password".to_string())
        .custom_sni(Some("cdn.example.org".to_string()))
        .has_ipv6(false)
        .upstream_protocol(Protocol::Http3)
        .anti_dpi(true)
        .build()
        .unwrap();

    let rust_uri = encode(&config).unwrap();
    let python_uri = python_encode(toml);

    assert_eq!(
        rust_uri, python_uri,
        "Rust and Python encoders produced different URIs for full config"
    );

    // Verify roundtrip through Python decoder
    let python_decoded = python_decode(&rust_uri);
    assert!(
        python_decoded.contains("hostname = \"secure.vpn.example.com\""),
        "Python decoder failed on hostname"
    );
    assert!(
        python_decoded.contains("custom_sni = \"cdn.example.org\""),
        "Python decoder failed on custom_sni"
    );
    assert!(
        python_decoded.contains("has_ipv6 = false"),
        "Python decoder failed on has_ipv6"
    );
    assert!(
        python_decoded.contains("upstream_protocol = \"http3\""),
        "Python decoder failed on upstream_protocol"
    );
    assert!(
        python_decoded.contains("anti_dpi = true"),
        "Python decoder failed on anti_dpi"
    );
}

#[test]
fn test_decode_python_encoded_uri() {
    let toml = r#"
hostname = "test.example.org"
addresses = ["203.0.113.1:9443"]
username = "testuser"
password = "testpass"
upstream_protocol = "http2"
"#;

    // Get Python-encoded URI
    let python_uri = python_encode(toml);

    // Decode with Rust
    let rust_config = decode(&python_uri).expect("Failed to decode Python-encoded URI");

    // Verify fields
    assert_eq!(rust_config.hostname.as_deref(), Some("test.example.org"));
    assert_eq!(rust_config.addresses.len(), 1);
    assert_eq!(rust_config.addresses[0], "203.0.113.1:9443");
    assert_eq!(rust_config.username.as_deref(), Some("testuser"));
    assert_eq!(rust_config.password.as_deref(), Some("testpass"));
    assert_eq!(rust_config.upstream_protocol, Protocol::Http2);
    assert!(rust_config.has_ipv6); // default
    assert!(!rust_config.anti_dpi); // default
}

#[test]
fn test_client_random_prefix_matches_python() {
    let toml = r#"
hostname = "crp.example.com"
addresses = ["10.20.30.40:8443"]
username = "testuser"
password = "testpass"
client_random_prefix = "aabbccddee"
"#;

    // Rust encode
    let config = DeepLinkConfig::builder()
        .hostname("crp.example.com".to_string())
        .addresses(vec!["10.20.30.40:8443".to_string()])
        .username("testuser".to_string())
        .password("testpass".to_string())
        .client_random_prefix(Some("aabbccddee".to_string()))
        .build()
        .unwrap();

    let rust_uri = encode(&config).unwrap();
    let python_uri = python_encode(toml);

    assert_eq!(
        rust_uri, python_uri,
        "Rust and Python encoders produced different URIs for client_random_prefix"
    );

    // Verify roundtrip through Python decoder
    let python_config_str = python_decode(&rust_uri);
    assert!(
        python_config_str.contains("client_random_prefix = \"aabbccddee\""),
        "Python decoder did not preserve client_random_prefix"
    );
}

#[test]
fn test_roundtrip_through_both_implementations() {
    // Start with Rust config
    let original_config = DeepLinkConfig::builder()
        .hostname("roundtrip.example.com".to_string())
        .addresses(vec!["198.51.100.1:443".to_string()])
        .username("roundtrip_user".to_string())
        .password("roundtrip_pass".to_string())
        .custom_sni(Some("sni.example.com".to_string()))
        .has_ipv6(false)
        .anti_dpi(true)
        .build()
        .unwrap();

    // Encode with Rust
    let rust_uri = encode(&original_config).unwrap();

    // Decode with Python (outputs TOML)
    let python_toml = python_decode(&rust_uri);

    // Re-encode with Python
    let temp_dir = std::env::temp_dir();
    let temp_file = temp_dir.join("roundtrip_config.toml");
    std::fs::write(&temp_file, &python_toml).expect("Failed to write temp TOML file");

    let python_uri = python_encode(&python_toml);
    std::fs::remove_file(&temp_file).ok();

    // URIs should match after roundtrip
    assert_eq!(
        rust_uri, python_uri,
        "URI changed after roundtrip through Python"
    );

    // Decode with Rust
    let decoded_config = decode(&python_uri).unwrap();

    // Verify all fields match
    assert_eq!(decoded_config.hostname, original_config.hostname);
    assert_eq!(decoded_config.addresses, original_config.addresses);
    assert_eq!(decoded_config.username, original_config.username);
    assert_eq!(decoded_config.password, original_config.password);
    assert_eq!(decoded_config.custom_sni, original_config.custom_sni);
    assert_eq!(decoded_config.has_ipv6, original_config.has_ipv6);
    assert_eq!(decoded_config.anti_dpi, original_config.anti_dpi);
    assert_eq!(
        decoded_config.upstream_protocol,
        original_config.upstream_protocol
    );
}

#[test]
fn test_name_and_dns_upstreams_matches_python() {
    let toml = r#"
hostname = "vpn.example.com"
addresses = ["1.2.3.4:443"]
username = "alice"
password = "secret123"
name = "My Server"
dns_upstreams = ["1.1.1.1", "8.8.8.8"]
"#;

    let config = DeepLinkConfig::builder()
        .hostname("vpn.example.com".to_string())
        .addresses(vec!["1.2.3.4:443".to_string()])
        .username("alice".to_string())
        .password("secret123".to_string())
        .name(Some("My Server".to_string()))
        .dns_upstreams(vec!["1.1.1.1".to_string(), "8.8.8.8".to_string()])
        .build()
        .unwrap();

    let rust_uri = encode(&config).unwrap();
    let python_uri = python_encode(toml);

    assert_eq!(
        rust_uri, python_uri,
        "Rust and Python encoders produced different URIs for name/dns_upstreams"
    );

    let python_decoded = python_decode(&rust_uri);
    assert!(
        python_decoded.contains("name = \"My Server\""),
        "Python decoder failed on name"
    );
    assert!(
        python_decoded.contains("dns_upstreams"),
        "Python decoder failed on dns_upstreams"
    );
}

#[test]
fn test_subscription_url_matches_python() {
    let toml = r#"
hostname = "vpn.example.com"
addresses = ["1.2.3.4:443"]
username = "alice"
password = "s3cret"
subscription_url = "https://alice:s3cret@vpn.example.com/subscription"
"#;

    let config = DeepLinkConfig::builder()
        .hostname("vpn.example.com".to_string())
        .addresses(vec!["1.2.3.4:443".to_string()])
        .username("alice".to_string())
        .password("s3cret".to_string())
        .subscription_url(Some(
            "https://alice:s3cret@vpn.example.com/subscription".to_string(),
        ))
        .build()
        .unwrap();

    let rust_uri = encode(&config).unwrap();
    let python_uri = python_encode(toml);

    assert_eq!(
        rust_uri, python_uri,
        "Rust and Python encoders produced different URIs for subscription_url"
    );

    let python_decoded = python_decode(&rust_uri);
    assert!(
        python_decoded
            .contains("subscription_url = \"https://alice:s3cret@vpn.example.com/subscription\""),
        "Python decoder failed on subscription_url"
    );
}

#[test]
fn test_subscription_only_matches_python() {
    let toml = r#"
subscription_url = "https://alice:s3cret@vpn.example.com/subscription"
"#;

    let config = DeepLinkConfig::builder()
        .subscription_url(Some(
            "https://alice:s3cret@vpn.example.com/subscription".to_string(),
        ))
        .build()
        .unwrap();

    let rust_uri = encode(&config).unwrap();
    let python_uri = python_encode(toml);

    assert_eq!(
        rust_uri, python_uri,
        "Rust and Python encoders produced different URIs for subscription-only config"
    );

    // Rust can decode the Python-encoded subscription-only URI.
    let decoded = decode(&python_uri).unwrap();
    assert_eq!(
        decoded.subscription_url.as_deref(),
        Some("https://alice:s3cret@vpn.example.com/subscription")
    );
    assert_eq!(decoded.hostname, None);
}
