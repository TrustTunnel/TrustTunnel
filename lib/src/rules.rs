use ipnet::IpNet;
use ring::hkdf::KeyType;
use serde::{Deserialize, Serialize};
use std::net::IpAddr;

/// Action to take when a rule matches
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum RuleAction {
    Allow,
    Deny,
}

/// Individual filter rule
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Rule {
    /// CIDR range to match against client IP
    #[serde(default)]
    pub cidr: Option<String>,

    /// Client random prefix to match (hex-encoded)
    /// Can optionally include a mask in format: "prefix[/mask]" (e.g., "aabbcc/ff00ff")
    /// If mask is specified, matching uses: client_random & mask == prefix & mask
    /// If no mask, uses prefix matching
    #[serde(default)]
    pub client_random_prefix: Option<String>,

    /// auth key (hex) for client_random validation via HKDF+AES.
    /// When set, matches() validates the full 32-byte client_random against the SNI.
    /// Mutually exclusive with client_random_prefix (the auth key takes priority).
    #[serde(default)]
    pub client_random_auth_key: Option<String>,

    /// Action to take when this rule matches
    pub action: RuleAction,
}

/// Rules configuration
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RulesConfig {
    /// List of filter rules
    #[serde(default)]
    pub rule: Vec<Rule>,
}

/// Rule evaluation engine
pub struct RulesEngine {
    rules: RulesConfig,
}

/// Result of rule evaluation
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuleEvaluation {
    Allow,
    Deny,
}

/// Key type for a 128-bit (16-byte) AES key produced by HKDF-Expand.
struct Aes128KeyLen;

impl KeyType for Aes128KeyLen {
    fn len(&self) -> usize {
        16
    }
}

/// Check if a string is valid even-length hex.
pub fn is_valid_hex(s: &str) -> bool {
    !s.is_empty() && s.len().is_multiple_of(2) && hex::decode(s).is_ok()
}

/// Validate a 32-byte `client_random` against `sni` using an auth-key-derived key.
///
/// The second 16 bytes of `client_random` must equal
/// AES-128-ECB(SHA256(SNI)[..16], HKDF-SHA256(auth_key, salt = random[..16],
/// info = "tls13 encryption context")).
pub fn validate_client_random_auth_key(auth_key: &[u8], client_random: &[u8], sni: &str) -> bool {
    if client_random.len() != 32 || sni.is_empty() || auth_key.is_empty() {
        return false;
    }
    let (random, ciphertext) = client_random.split_at(16);

    // HKDF-SHA256(secret = auth_key, salt = random, info = "tls13 encryption context") -> 16-byte key
    let info: [&[u8]; 1] = [b"tls13 encryption context"];
    let salt = ring::hkdf::Salt::new(ring::hkdf::HKDF_SHA256, random);
    let prk = salt.extract(auth_key);
    let okm = match prk.expand(&info, Aes128KeyLen) {
        Ok(okm) => okm,
        Err(_) => return false,
    };
    let mut derived_key = [0u8; 16];
    if okm.fill(&mut derived_key).is_err() {
        return false;
    }

    let sni_hash = ring::digest::digest(&ring::digest::SHA256, sni.as_bytes());
    let sni_hash_prefix = &sni_hash.as_ref()[..16];

    // AES-128-ECB encrypt of the first 16 bytes of SHA256(SNI), no padding
    let mut crypter = match boring::symm::Crypter::new(
        boring::symm::Cipher::aes_128_ecb(),
        boring::symm::Mode::Encrypt,
        &derived_key,
        None,
    ) {
        Ok(c) => c,
        Err(_) => return false,
    };
    crypter.pad(false);
    // output buffer must be at least input.len() + block_size() = 32
    let mut buf = [0u8; 32];
    let n = match crypter.update(sni_hash_prefix, &mut buf) {
        Ok(n) => n,
        Err(_) => return false,
    };
    if crypter.finalize(&mut buf[n..]).is_err() {
        return false;
    }
    let expected = &buf[..n.min(16)];

    // Constant-time comparison to avoid timing side channels
    let mut diff = 0u8;
    for i in 0..expected.len() {
        diff |= ciphertext[i] ^ expected[i];
    }
    diff == 0
}

impl Rule {
    /// Check if this rule matches the given connection parameters
    pub fn matches(
        &self,
        client_ip: &IpAddr,
        client_random: Option<&[u8]>,
        sni: Option<&str>,
    ) -> bool {
        let mut matches = true;

        // Check CIDR match if specified
        if let Some(cidr_str) = &self.cidr {
            if let Ok(cidr) = cidr_str.parse::<IpNet>() {
                matches &= cidr.contains(client_ip);
            } else {
                // Invalid CIDR, rule doesn't match
                return false;
            }
        }

        // Check client_random auth key if specified (takes priority over prefix)
        if let Some(auth_hex) = &self.client_random_auth_key {
            if let (Some(client_random_data), Some(sni_str)) = (client_random, sni) {
                if let Ok(auth_bytes) = hex::decode(auth_hex) {
                    let auth_valid =
                        validate_client_random_auth_key(&auth_bytes, client_random_data, sni_str);
                    if !auth_valid {
                        log::debug!(
                            "auth key client_random validation failed for SNI: {}",
                            crate::net_utils::scrub_sni(sni_str.to_string())
                        );
                    }
                    matches &= auth_valid;
                } else {
                    // Invalid hex auth_key key in rule, rule doesn't match
                    matches = false;
                }
            } else {
                // No client_random or sni provided but rule requires it, doesn't match
                matches = false;
            }
        } else if let Some(prefix_str) = &self.client_random_prefix {
            // Check client_random prefix if specified
            if let Some(client_random_data) = client_random {
                // Check if mask is specified in format "prefix[/mask]"
                if let Some(slash_pos) = prefix_str.find('/') {
                    // Parse prefix and mask separately
                    let (prefix_part, mask_part) = prefix_str.split_at(slash_pos);
                    let mask_part = &mask_part[1..]; // Skip the '/'

                    if let (Ok(prefix_bytes), Ok(mask_bytes)) =
                        (hex::decode(prefix_part), hex::decode(mask_part))
                    {
                        // Apply mask: client_random & mask == prefix & mask
                        let mask_len = mask_bytes
                            .len()
                            .min(prefix_bytes.len())
                            .min(client_random_data.len());
                        let mut masked_match = mask_len > 0;

                        for i in 0..mask_len {
                            if (client_random_data[i] & mask_bytes[i])
                                != (prefix_bytes[i] & mask_bytes[i])
                            {
                                masked_match = false;
                                break;
                            }
                        }

                        matches &= masked_match;
                    } else {
                        // Invalid hex in prefix or mask, rule doesn't match
                        return false;
                    }
                } else {
                    // No mask, use simple prefix matching
                    if let Ok(prefix_bytes) = hex::decode(prefix_str) {
                        matches &= client_random_data.starts_with(&prefix_bytes);
                    } else {
                        // Invalid hex prefix, rule doesn't match
                        return false;
                    }
                }
            } else {
                // No client_random provided but rule requires it, doesn't match
                matches = false;
            }
        }

        matches
    }
}

impl RulesEngine {
    /// Create a new rules engine from rules config
    pub fn from_config(rules: RulesConfig) -> Self {
        Self { rules }
    }

    /// Create a default rules engine that allows all connections
    pub fn default_allow() -> Self {
        Self {
            rules: RulesConfig { rule: vec![] },
        }
    }

    /// Evaluate connection against all rules
    /// Returns the action from the first matching rule, or Allow if no rules match
    pub fn evaluate(
        &self,
        client_ip: &IpAddr,
        client_random: Option<&[u8]>,
        sni: Option<&str>,
    ) -> RuleEvaluation {
        let has_prefix_rule = self
            .rules
            .rule
            .iter()
            .any(|r| r.client_random_prefix.is_some());
        let has_auth_key_rule = self
            .rules
            .rule
            .iter()
            .any(|r| r.client_random_auth_key.is_some());

        let sni_is_missing = sni.is_none_or(|s| s.is_empty());

        if (client_random.is_none() && (has_prefix_rule || has_auth_key_rule))
            || (sni_is_missing && has_auth_key_rule)
        {
            return RuleEvaluation::Deny;
        }

        for rule in &self.rules.rule {
            if rule.matches(client_ip, client_random, sni) {
                return match rule.action {
                    RuleAction::Allow => RuleEvaluation::Allow,
                    RuleAction::Deny => RuleEvaluation::Deny,
                };
            }
        }

        // Default action if no rules match: allow
        RuleEvaluation::Allow
    }

    /// Get a reference to the rules configuration
    pub fn config(&self) -> &RulesConfig {
        &self.rules
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    /// Derive a deterministic 32-byte client_random for an auth key and SNI, mirroring
    /// the algorithm verified by validate_client_random_auth_key.
    fn derive_client_random(auth_key: &[u8], sni: &str) -> Vec<u8> {
        let random = [0x11u8; 16];
        let info: [&[u8]; 1] = [b"tls13 encryption context"];
        let salt = ring::hkdf::Salt::new(ring::hkdf::HKDF_SHA256, &random);
        let prk = salt.extract(auth_key);
        let okm = prk.expand(&info, Aes128KeyLen).unwrap();
        let mut derived_key = [0u8; 16];
        okm.fill(&mut derived_key).unwrap();

        let sni_hash = ring::digest::digest(&ring::digest::SHA256, sni.as_bytes());
        let mut crypter = boring::symm::Crypter::new(
            boring::symm::Cipher::aes_128_ecb(),
            boring::symm::Mode::Encrypt,
            &derived_key,
            None,
        )
        .unwrap();
        crypter.pad(false);
        let mut buf = [0u8; 32];
        let n = crypter.update(&sni_hash.as_ref()[..16], &mut buf).unwrap();
        crypter.finalize(&mut buf[n..]).unwrap();

        let mut client_random = Vec::with_capacity(32);
        client_random.extend_from_slice(&random);
        client_random.extend_from_slice(&buf[..16]);
        client_random
    }

    #[test]
    fn test_cidr_rule_matching() {
        let rule = Rule {
            cidr: Some("192.168.1.0/24".to_string()),
            client_random_prefix: None,
            client_random_auth_key: None,
            action: RuleAction::Allow,
        };

        let ip_match = IpAddr::from_str("192.168.1.100").unwrap();
        let ip_no_match = IpAddr::from_str("10.0.0.1").unwrap();

        assert!(rule.matches(&ip_match, None, None));
        assert!(!rule.matches(&ip_no_match, None, None));
    }

    #[test]
    fn test_client_random_prefix_matching() {
        let rule = Rule {
            cidr: None,
            client_random_prefix: Some("aabbcc".to_string()),
            client_random_auth_key: None,
            action: RuleAction::Deny,
        };

        let client_random_match = hex::decode("aabbccddee").unwrap();
        let client_random_no_match = hex::decode("112233").unwrap();

        let ip = IpAddr::from_str("127.0.0.1").unwrap();

        assert!(rule.matches(&ip, Some(&client_random_match), None));
        assert!(!rule.matches(&ip, Some(&client_random_no_match), None));
        assert!(!rule.matches(&ip, None, None)); // No client random provided
    }

    #[test]
    fn test_combined_rule_matching() {
        let rule = Rule {
            cidr: Some("10.0.0.0/8".to_string()),
            client_random_prefix: Some("ff".to_string()),
            client_random_auth_key: None,
            action: RuleAction::Allow,
        };

        let ip_match = IpAddr::from_str("10.1.2.3").unwrap();
        let ip_no_match = IpAddr::from_str("192.168.1.1").unwrap();
        let client_random_match = hex::decode("ff00112233").unwrap();
        let client_random_no_match = hex::decode("0011223344").unwrap();

        // Both must match
        assert!(rule.matches(&ip_match, Some(&client_random_match), None));
        assert!(!rule.matches(&ip_match, Some(&client_random_no_match), None));
        assert!(!rule.matches(&ip_no_match, Some(&client_random_match), None));
        assert!(!rule.matches(&ip_no_match, Some(&client_random_no_match), None));
    }

    #[test]
    fn test_rules_engine_evaluation() {
        let rules = RulesConfig {
            rule: vec![
                Rule {
                    cidr: Some("192.168.1.0/24".to_string()),
                    client_random_prefix: None,
                    client_random_auth_key: None,
                    action: RuleAction::Deny,
                },
                Rule {
                    cidr: Some("10.0.0.0/8".to_string()),
                    client_random_prefix: None,
                    client_random_auth_key: None,
                    action: RuleAction::Allow,
                },
                Rule {
                    cidr: None,
                    client_random_prefix: None,
                    client_random_auth_key: None,
                    action: RuleAction::Deny, // Catch-all deny
                },
            ],
        };

        let engine = RulesEngine::from_config(rules);

        let ip_deny = IpAddr::from_str("192.168.1.100").unwrap();
        let ip_allow = IpAddr::from_str("10.1.2.3").unwrap();
        let ip_default = IpAddr::from_str("172.16.1.1").unwrap();

        assert_eq!(engine.evaluate(&ip_deny, None, None), RuleEvaluation::Deny);
        assert_eq!(
            engine.evaluate(&ip_allow, None, None),
            RuleEvaluation::Allow
        );
        assert_eq!(
            engine.evaluate(&ip_default, None, None),
            RuleEvaluation::Deny
        ); // Default deny
    }

    #[test]
    fn test_rules_engine_fails_closed_without_client_random() {
        let rules = RulesConfig {
            rule: vec![Rule {
                cidr: None,
                client_random_prefix: Some("aabbcc".to_string()),
                client_random_auth_key: None,
                action: RuleAction::Allow,
            }],
        };

        let engine = RulesEngine::from_config(rules);
        let ip = IpAddr::from_str("127.0.0.1").unwrap();

        assert_eq!(engine.evaluate(&ip, None, None), RuleEvaluation::Deny);
    }

    #[test]
    fn test_client_random_mask_matching() {
        // Test mask matching: only check specific bits
        // Format: "prefix/mask" where mask 0xf0f0 means we only care about bits in positions where mask is 1
        let rule = Rule {
            cidr: None,
            client_random_prefix: Some("a0b0/f0f0".to_string()), // prefix=a0b0, mask=f0f0
            client_random_auth_key: None,
            action: RuleAction::Allow,
        };

        let ip = IpAddr::from_str("127.0.0.1").unwrap();

        // Should match: a5b5 & f0f0 = a0b0, same as prefix & mask
        let client_random_match1 = hex::decode("a5b5ccdd").unwrap(); // 10100101 10110101
                                                                     // Should match: a9bf & f0f0 = a0b0, same as prefix & mask
        let client_random_match2 = hex::decode("a9bfeeaa").unwrap(); // 10101001 10111111
                                                                     // Should not match: b0b0 & f0f0 = b0b0, different from a0b0
        let client_random_no_match1 = hex::decode("b0b01122").unwrap(); // 10110000 10110000
                                                                        // Should not match: a0c0 & f0f0 = a0c0, different from a0b0
        let client_random_no_match2 = hex::decode("a0c03344").unwrap(); // 10100000 11000000

        assert!(rule.matches(&ip, Some(&client_random_match1), None));
        assert!(rule.matches(&ip, Some(&client_random_match2), None));
        assert!(!rule.matches(&ip, Some(&client_random_no_match1), None));
        assert!(!rule.matches(&ip, Some(&client_random_no_match2), None));
    }

    #[test]
    fn test_client_random_mask_full_bytes() {
        // Test with full byte mask - only first 2 bytes matter
        let rule = Rule {
            cidr: None,
            client_random_prefix: Some("12345678/ffff0000".to_string()),
            client_random_auth_key: None,
            action: RuleAction::Allow,
        };

        let ip = IpAddr::from_str("127.0.0.1").unwrap();

        // Should match: first 2 bytes are 0x1234, last 2 can be anything
        let client_random_match = hex::decode("1234aaaabbbb").unwrap();
        // Should not match: first 2 bytes are 0x1233
        let client_random_no_match = hex::decode("12335678ccdd").unwrap();

        assert!(rule.matches(&ip, Some(&client_random_match), None));
        assert!(!rule.matches(&ip, Some(&client_random_no_match), None));
    }

    #[test]
    fn test_client_random_invalid_mask_format() {
        // Test that invalid format "prefix/" (slash without mask) doesn't match
        let rule = Rule {
            cidr: None,
            client_random_prefix: Some("aabbcc/".to_string()), // Invalid: empty mask
            client_random_auth_key: None,
            action: RuleAction::Allow,
        };

        let ip = IpAddr::from_str("127.0.0.1").unwrap();
        let client_random = hex::decode("aabbccddee").unwrap();

        // Should not match due to invalid format
        assert!(!rule.matches(&ip, Some(&client_random), None));
    }

    #[test]
    fn test_auth_key_validation_accepts_derived_client_random() {
        let auth_key = hex::decode("aabbccddeeff00112233445566778899").unwrap();
        let client_random = derive_client_random(&auth_key, "test.example.com");

        assert!(validate_client_random_auth_key(
            &auth_key,
            &client_random,
            "test.example.com"
        ));
    }

    #[test]
    fn test_auth_key_validation_hardcoded_vector_abcd() {
        let auth_key = hex::decode("aabbccddeeff00112233445566778899").unwrap();
        let client_random =
            hex::decode("112233445566778899001122334455669591807266c901dfe8c73c3040b1bd82")
                .unwrap();

        assert!(validate_client_random_auth_key(
            &auth_key,
            &client_random,
            "abcd"
        ));
    }

    #[test]
    fn test_auth_key_validation_hardcoded_vector_word_tips() {
        let auth_key = hex::decode("00112233445566778899aabbccddeeff").unwrap();
        let client_random =
            hex::decode("ffeeddccbbaa998877665544332211001fdc0ae97cd36d557dd426c34566e234")
                .unwrap();

        assert!(validate_client_random_auth_key(
            &auth_key,
            &client_random,
            "word.tips"
        ));
    }

    #[test]
    fn test_auth_key_validation_wrong_sni() {
        let auth_key = hex::decode("aabbccddeeff00112233445566778899").unwrap();
        let client_random = derive_client_random(&auth_key, "a.example.com");

        assert!(!validate_client_random_auth_key(
            &auth_key,
            &client_random,
            "b.example.com"
        ));
    }

    #[test]
    fn test_auth_key_validation_wrong_auth_key() {
        let auth_key1 = hex::decode("aabbccddeeff00112233445566778899").unwrap();
        let auth_key2 = hex::decode("00112233445566778899aabbccddeeff").unwrap();
        let client_random = derive_client_random(&auth_key1, "test.example.com");

        assert!(!validate_client_random_auth_key(
            &auth_key2,
            &client_random,
            "test.example.com"
        ));
    }

    #[test]
    fn test_auth_key_validation_short_client_random() {
        let auth_key = hex::decode("aabbccddeeff00112233445566778899").unwrap();
        let client_random = vec![0u8; 16];

        assert!(!validate_client_random_auth_key(
            &auth_key,
            &client_random,
            "test.example.com"
        ));
    }

    #[test]
    fn test_rule_matches_auth_key_takes_priority_over_prefix() {
        let auth_key = hex::decode("aabbccddeeff00112233445566778899").unwrap();
        let client_random = derive_client_random(&auth_key, "test.example.com");
        // Prefix does NOT match the derived client_random, auth key does.
        let rule = Rule {
            cidr: None,
            client_random_prefix: Some("ffff".to_string()),
            client_random_auth_key: Some("aabbccddeeff00112233445566778899".to_string()),
            action: RuleAction::Allow,
        };

        let ip = IpAddr::from_str("127.0.0.1").unwrap();

        assert!(rule.matches(&ip, Some(&client_random), Some("test.example.com")));
    }

    #[test]
    fn test_rule_matches_auth_key_requires_sni() {
        let auth_key = hex::decode("aabbccddeeff00112233445566778899").unwrap();
        let client_random = derive_client_random(&auth_key, "test.example.com");
        let rule = Rule {
            cidr: None,
            client_random_prefix: None,
            client_random_auth_key: Some("aabbccddeeff00112233445566778899".to_string()),
            action: RuleAction::Allow,
        };

        let ip = IpAddr::from_str("127.0.0.1").unwrap();

        // No SNI provided -> auth key cannot be validated -> rule does not match
        assert!(!rule.matches(&ip, Some(&client_random), None));
    }

    #[test]
    fn test_engine_fails_closed_when_client_random_missing_for_auth_key() {
        let rules = RulesConfig {
            rule: vec![Rule {
                cidr: None,
                client_random_prefix: None,
                client_random_auth_key: Some("aabbccddeeff00112233445566778899".to_string()),
                action: RuleAction::Allow,
            }],
        };

        let engine = RulesEngine::from_config(rules);
        let ip = IpAddr::from_str("127.0.0.1").unwrap();
        let client_random = derive_client_random(
            &hex::decode("aabbccddeeff00112233445566778899").unwrap(),
            "test.example.com",
        );

        // Missing client_random -> deny (defense-in-depth)
        assert_eq!(
            engine.evaluate(&ip, None, Some("test.example.com")),
            RuleEvaluation::Deny
        );
        // Missing sni -> deny
        assert_eq!(
            engine.evaluate(&ip, Some(&client_random), None),
            RuleEvaluation::Deny
        );
        // Valid auth key handshake -> allow
        assert_eq!(
            engine.evaluate(&ip, Some(&client_random), Some("test.example.com")),
            RuleEvaluation::Allow
        );
    }

    #[test]
    fn test_engine_auth_key_with_cidr_both_must_match() {
        let rule = Rule {
            cidr: Some("192.168.1.0/24".to_string()),
            client_random_prefix: None,
            client_random_auth_key: Some("aabbccddeeff00112233445566778899".to_string()),
            action: RuleAction::Allow,
        };
        let auth_key = hex::decode("aabbccddeeff00112233445566778899").unwrap();
        let client_random = derive_client_random(&auth_key, "test.example.com");

        let ip_match = IpAddr::from_str("192.168.1.50").unwrap();
        let ip_no_match = IpAddr::from_str("10.0.0.5").unwrap();

        // CIDR and auth key are AND: the rule matches only when both are satisfied
        assert!(rule.matches(&ip_match, Some(&client_random), Some("test.example.com")));
        assert!(!rule.matches(&ip_no_match, Some(&client_random), Some("test.example.com")));
    }

    #[test]
    fn test_engine_empty_sni_denied_for_auth_key_rules() {
        let rules = RulesConfig {
            rule: vec![Rule {
                cidr: None,
                client_random_prefix: None,
                client_random_auth_key: Some("aabbccddeeff00112233445566778899".to_string()),
                action: RuleAction::Allow,
            }],
        };
        let engine = RulesEngine::from_config(rules);
        let ip = IpAddr::from_str("127.0.0.1").unwrap();
        let client_random = derive_client_random(
            &hex::decode("aabbccddeeff00112233445566778899").unwrap(),
            "test.example.com",
        );

        // Empty SNI (QUIC bootstrap case) → Deny
        assert_eq!(
            engine.evaluate(&ip, Some(&client_random), Some("")),
            RuleEvaluation::Deny
        );
    }
}
