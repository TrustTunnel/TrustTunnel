use crate::get_mode;
use crate::user_interaction::{ask_for_agreement, ask_for_input};
use log::{info, warn};
use trusttunnel::rules::{Rule, RuleAction, RulesConfig};

pub fn build() -> RulesConfig {
    match get_mode() {
        crate::Mode::NonInteractive => build_non_interactive(),
        crate::Mode::Interactive => build_interactive(),
    }
}

fn build_non_interactive() -> RulesConfig {
    // In non-interactive mode, generate empty rules
    // The actual examples will be in the serialized TOML comments
    RulesConfig { rule: vec![] }
}

fn build_interactive() -> RulesConfig {
    info!("Setting up connection filtering rules...");

    let mut rules = Vec::new();

    // Ask if user wants to configure rules
    if !ask_for_agreement("Do you want to configure connection filtering rules? (if not, all connections will be allowed)") {
        info!("Skipping rules configuration - all connections will be allowed.");
        return RulesConfig { rule: vec![] };
    }

    println!();
    println!("You can configure rules to allow/deny connections based on:");
    println!("  - Client IP address (CIDR notation, e.g., 192.168.1.0/24)");
    println!("  - TLS client random prefix (hex-encoded, e.g., aabbcc)");
    println!("  - TLS client random with mask for bitwise matching");
    println!("  - Both conditions together");
    println!();

    add_custom_rules(&mut rules);

    RulesConfig { rule: rules }
}

fn add_custom_rules(rules: &mut Vec<Rule>) {
    println!();
    while ask_for_agreement("Add a custom rule?") {
        let rule_type = ask_for_input::<String>(
            "Rule type (1=IP range, 2=client random prefix, 3=CIDR+prefix, 4=PSK key, 5=CIDR+PSK key)",
            Some("1".to_string()),
        );

        match rule_type.as_str() {
            "1" => add_ip_rule(rules),
            "2" => add_client_random_rule(rules),
            "3" => add_combined_rule(rules),
            "4" => add_psk_rule(rules),
            "5" => add_combined_psk_rule(rules),
            _ => {
                warn!("Invalid choice. Skipping rule.");
                continue;
            }
        }
        println!();
    }
}

fn add_ip_rule(rules: &mut Vec<Rule>) {
    let cidr = ask_for_input::<String>(
        "Enter IP range in CIDR notation (e.g., 203.0.113.0/24)",
        None,
    );

    // Validate CIDR format
    if cidr.parse::<ipnet::IpNet>().is_err() {
        warn!("Invalid CIDR format. Skipping rule.");
        return;
    }

    let action = ask_for_rule_action();

    rules.push(Rule {
        cidr: Some(cidr),
        client_random_prefix: None,
        client_random_psk_key: None,
        action,
    });

    info!("Rule added successfully.");
}

fn add_client_random_rule(rules: &mut Vec<Rule>) {
    let client_random_value = ask_for_input::<String>(
        "Enter client random prefix (hex, format: prefix[/mask], e.g., aabbcc/ffff0000)",
        None,
    );

    // Validate format
    if let Some(slash_pos) = client_random_value.find('/') {
        // Format: prefix/mask
        let (prefix_part, mask_part) = client_random_value.split_at(slash_pos);
        let mask_part = &mask_part[1..]; // Skip the '/'

        if mask_part.is_empty() {
            warn!("Invalid format: mask is empty after '/'. Skipping rule.");
            return;
        }

        // Validate both prefix and mask are valid hex
        if hex::decode(prefix_part).is_err() {
            warn!("Invalid hex format in prefix part. Skipping rule.");
            return;
        }

        if hex::decode(mask_part).is_err() {
            warn!("Invalid hex format in mask part. Skipping rule.");
            return;
        }
    } else {
        // Format: just prefix
        if hex::decode(&client_random_value).is_err() {
            warn!("Invalid hex format. Skipping rule.");
            return;
        }
    }

    let action = ask_for_rule_action();

    rules.push(Rule {
        cidr: None,
        client_random_prefix: Some(client_random_value),
        client_random_psk_key: None,
        action,
    });

    info!("Rule added successfully.");
}

fn add_combined_rule(rules: &mut Vec<Rule>) {
    let cidr = ask_for_input::<String>(
        "Enter IP range in CIDR notation (e.g., 172.16.0.0/12)",
        None,
    );

    // Validate CIDR format
    if cidr.parse::<ipnet::IpNet>().is_err() {
        warn!("Invalid CIDR format. Skipping rule.");
        return;
    }

    let client_random_value = ask_for_input::<String>(
        "Enter client random prefix (hex, format: prefix or prefix/mask, e.g., 001122 or 001122/ffff00)",
        None,
    );

    // Validate format
    if let Some(slash_pos) = client_random_value.find('/') {
        // Format: prefix/mask
        let (prefix_part, mask_part) = client_random_value.split_at(slash_pos);
        let mask_part = &mask_part[1..]; // Skip the '/'

        if mask_part.is_empty() {
            warn!("Invalid format: mask is empty after '/'. Skipping rule.");
            return;
        }

        // Validate both prefix and mask are valid hex
        if hex::decode(prefix_part).is_err() {
            warn!("Invalid hex format in prefix part. Skipping rule.");
            return;
        }

        if hex::decode(mask_part).is_err() {
            warn!("Invalid hex format in mask part. Skipping rule.");
            return;
        }
    } else {
        // Format: just prefix
        if hex::decode(&client_random_value).is_err() {
            warn!("Invalid hex format. Skipping rule.");
            return;
        }
    }

    let action = ask_for_rule_action();

    rules.push(Rule {
        cidr: Some(cidr),
        client_random_prefix: Some(client_random_value),
        client_random_psk_key: None,
        action,
    });

    info!("Rule added successfully.");
}

fn ask_for_rule_action() -> RuleAction {
    let action_str = ask_for_input::<String>("Action (allow/deny)", Some("allow".to_string()));

    match action_str.to_lowercase().as_str() {
        "deny" => RuleAction::Deny,
        _ => RuleAction::Allow,
    }
}

fn add_psk_rule(rules: &mut Vec<Rule>) {
    let psk_key = ask_for_input::<String>(
        "Enter client random PSK key (hex, 32+ chars recommended, e.g., aabbccddeeff00112233445566778899)",
        None,
    );

    if !validate_psk(&psk_key) {
        return;
    }

    let action = ask_for_rule_action();

    rules.push(Rule {
        cidr: None,
        client_random_prefix: None,
        client_random_psk_key: Some(psk_key),
        action,
    });

    info!("Rule added successfully.");
}

fn add_combined_psk_rule(rules: &mut Vec<Rule>) {
    let cidr = ask_for_input::<String>(
        "Enter IP range in CIDR notation (e.g., 172.16.0.0/12)",
        None,
    );

    if cidr.parse::<ipnet::IpNet>().is_err() {
        warn!("Invalid CIDR format. Skipping rule.");
        return;
    }

    let psk_key = ask_for_input::<String>(
        "Enter client random PSK key (hex, 32+ chars recommended, e.g., aabbccddeeff00112233445566778899)",
        None,
    );

    if !validate_psk(&psk_key) {
        return;
    }

    let action = ask_for_rule_action();

    rules.push(Rule {
        cidr: Some(cidr),
        client_random_prefix: None,
        client_random_psk_key: Some(psk_key),
        action,
    });

    info!("Rule added successfully.");
}

fn validate_psk(psk: &str) -> bool {
    if !trusttunnel::rules::is_valid_hex(psk) {
        warn!("Invalid hex format for PSK key. Skipping rule.");
        return false;
    }
    if psk.len() < 32 {
        warn!(
            "PSK key is shorter than 16 bytes (32 hex chars); \
             a short PSK weakens authentication. Skipping rule."
        );
        return false;
    }
    true
}
