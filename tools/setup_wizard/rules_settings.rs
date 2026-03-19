use crate::get_mode;
use crate::user_interaction::{ask_for_agreement, ask_for_input};
use log::{info, warn};
use trusttunnel::rules::{
    DestinationPortFilter, InboundRule, InboundRulesConfig, OutboundRule, OutboundRulesConfig,
    RuleAction, RulesConfig,
};

pub fn build() -> RulesConfig {
    match get_mode() {
        crate::Mode::NonInteractive => build_non_interactive(),
        crate::Mode::Interactive => build_interactive(),
    }
}

fn build_non_interactive() -> RulesConfig {
    RulesConfig::default()
}

fn build_interactive() -> RulesConfig {
    info!("Setting up connection filtering rules...");

    if !ask_for_agreement("Do you want to configure connection filtering rules? (if not, all connections will be allowed)") {
        info!("Skipping rules configuration - all connections will be allowed.");
        return RulesConfig::default();
    }

    println!();
    println!("Rules are split into two sections:");
    println!("  [inbound]  - Client filtering (evaluated at TLS handshake)");
    println!("    - Client IP address (CIDR notation, e.g., 192.168.1.0/24)");
    println!("    - TLS client random prefix (hex-encoded, e.g., aabbcc)");
    println!("    - TLS client random with mask for bitwise matching");
    println!("  [outbound] - Destination filtering (evaluated per request)");
    println!("    - Destination port or port range (e.g., 6881-6889)");
    println!("    - Destination IP range in CIDR notation (e.g., 10.0.0.0/8)");
    println!("    - Both port and IP (both must match)");
    println!();

    let inbound = build_inbound_section();
    let outbound = build_outbound_section();

    RulesConfig { inbound, outbound }
}

fn build_inbound_section() -> InboundRulesConfig {
    println!("--- Inbound rules (client filtering) ---");

    let default_action = ask_for_default_action("inbound");
    let mut rules = Vec::new();

    add_inbound_rules(&mut rules);

    InboundRulesConfig {
        default_action,
        rule: rules,
    }
}

fn build_outbound_section() -> OutboundRulesConfig {
    println!();
    println!("--- Outbound rules (destination filtering) ---");

    let default_action = ask_for_default_action("outbound");
    let mut rules = Vec::new();

    add_outbound_rules(&mut rules);

    OutboundRulesConfig {
        default_action,
        rule: rules,
    }
}

fn ask_for_default_action(section: &str) -> Option<RuleAction> {
    let action_str = ask_for_input::<String>(
        &format!(
            "Default action for {} when no rules match (allow/deny, leave empty for allow)",
            section
        ),
        Some("allow".to_string()),
    );

    match action_str.to_lowercase().as_str() {
        "deny" => Some(RuleAction::Deny),
        _ => None, // None means default allow
    }
}

fn add_inbound_rules(rules: &mut Vec<InboundRule>) {
    while ask_for_agreement("Add an inbound rule?") {
        let rule_type = ask_for_input::<String>(
            "Rule type (1=IP range, 2=client random prefix, 3=both)",
            Some("1".to_string()),
        );

        match rule_type.as_str() {
            "1" => add_ip_rule(rules),
            "2" => add_client_random_rule(rules),
            "3" => add_combined_rule(rules),
            _ => {
                warn!("Invalid choice. Skipping rule.");
                continue;
            }
        }
        println!();
    }
}

fn add_outbound_rules(rules: &mut Vec<OutboundRule>) {
    while ask_for_agreement("Add an outbound rule?") {
        let rule_type = ask_for_input::<String>(
            "Rule type (1=destination port, 2=destination IP range, 3=both)",
            Some("1".to_string()),
        );

        match rule_type.as_str() {
            "1" => add_destination_port_rule(rules),
            "2" => add_destination_cidr_rule(rules),
            "3" => add_destination_combined_rule(rules),
            _ => {
                warn!("Invalid choice. Skipping rule.");
                continue;
            }
        }
        println!();
    }
}

fn add_ip_rule(rules: &mut Vec<InboundRule>) {
    let cidr = ask_for_input::<String>(
        "Enter IP range in CIDR notation (e.g., 203.0.113.0/24)",
        None,
    );

    if cidr.parse::<ipnet::IpNet>().is_err() {
        warn!("Invalid CIDR format. Skipping rule.");
        return;
    }

    let action = ask_for_rule_action();

    rules.push(InboundRule {
        cidr: Some(cidr),
        client_random_prefix: None,
        action,
    });

    info!("Rule added successfully.");
}

fn add_client_random_rule(rules: &mut Vec<InboundRule>) {
    let client_random_value = ask_for_input::<String>(
        "Enter client random prefix (hex, format: prefix[/mask], e.g., aabbcc/ffff0000)",
        None,
    );

    if !validate_client_random(&client_random_value) {
        return;
    }

    let action = ask_for_rule_action();

    rules.push(InboundRule {
        cidr: None,
        client_random_prefix: Some(client_random_value),
        action,
    });

    info!("Rule added successfully.");
}

fn add_combined_rule(rules: &mut Vec<InboundRule>) {
    let cidr = ask_for_input::<String>(
        "Enter IP range in CIDR notation (e.g., 172.16.0.0/12)",
        None,
    );

    if cidr.parse::<ipnet::IpNet>().is_err() {
        warn!("Invalid CIDR format. Skipping rule.");
        return;
    }

    let client_random_value = ask_for_input::<String>(
        "Enter client random prefix (hex, format: prefix or prefix/mask, e.g., 001122 or 001122/ffff00)",
        None,
    );

    if !validate_client_random(&client_random_value) {
        return;
    }

    let action = ask_for_rule_action();

    rules.push(InboundRule {
        cidr: Some(cidr),
        client_random_prefix: Some(client_random_value),
        action,
    });

    info!("Rule added successfully.");
}

fn add_destination_port_rule(rules: &mut Vec<OutboundRule>) {
    let port_str = ask_for_input::<String>(
        "Enter destination port or range (e.g., 6881 or 6881-6889)",
        None,
    );

    let destination_port = match DestinationPortFilter::parse(&port_str) {
        Ok(filter) => filter,
        Err(e) => {
            warn!("Invalid port format: {}. Skipping rule.", e);
            return;
        }
    };

    let action = ask_for_rule_action();

    rules.push(OutboundRule {
        destination_port: Some(destination_port),
        destination_cidr: None,
        action,
    });

    info!("Rule added successfully.");
}

fn add_destination_cidr_rule(rules: &mut Vec<OutboundRule>) {
    let cidr_str = ask_for_input::<String>(
        "Enter destination IP range in CIDR notation (e.g., 10.0.0.0/8)",
        None,
    );

    let cidr = match cidr_str.parse::<ipnet::IpNet>() {
        Ok(c) => c,
        Err(_) => {
            warn!("Invalid CIDR format. Skipping rule.");
            return;
        }
    };

    let action = ask_for_rule_action();

    rules.push(OutboundRule {
        destination_port: None,
        destination_cidr: Some(cidr),
        action,
    });

    info!("Rule added successfully.");
}

fn add_destination_combined_rule(rules: &mut Vec<OutboundRule>) {
    let cidr_str = ask_for_input::<String>(
        "Enter destination IP range in CIDR notation (e.g., 203.0.113.0/24)",
        None,
    );

    let cidr = match cidr_str.parse::<ipnet::IpNet>() {
        Ok(c) => c,
        Err(_) => {
            warn!("Invalid CIDR format. Skipping rule.");
            return;
        }
    };

    let port_str = ask_for_input::<String>(
        "Enter destination port or range (e.g., 25 or 6881-6889)",
        None,
    );

    let destination_port = match DestinationPortFilter::parse(&port_str) {
        Ok(filter) => filter,
        Err(e) => {
            warn!("Invalid port format: {}. Skipping rule.", e);
            return;
        }
    };

    let action = ask_for_rule_action();

    rules.push(OutboundRule {
        destination_port: Some(destination_port),
        destination_cidr: Some(cidr),
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

fn validate_client_random(value: &str) -> bool {
    if let Some(slash_pos) = value.find('/') {
        let (prefix_part, mask_part) = value.split_at(slash_pos);
        let mask_part = &mask_part[1..];

        if mask_part.is_empty() {
            warn!("Invalid format: mask is empty after '/'. Skipping rule.");
            return false;
        }

        if hex::decode(prefix_part).is_err() {
            warn!("Invalid hex format in prefix part. Skipping rule.");
            return false;
        }

        if hex::decode(mask_part).is_err() {
            warn!("Invalid hex format in mask part. Skipping rule.");
            return false;
        }
    } else if hex::decode(value).is_err() {
        warn!("Invalid hex format. Skipping rule.");
        return false;
    }

    true
}
