use crate::user_interaction::{
    ask_for_agreement, ask_for_input, ask_for_password, checked_overwrite, select_variant,
};
use crate::Mode;
use std::fs;
use toml_edit::{ArrayOfTables, Item, Key, Table};
use trusttunnel::authentication::registry_based::Client;
use trusttunnel::settings::{
    Http1Settings, Http2Settings, ListenProtocolSettings, QuicSettings, Settings,
};

pub const DEFAULT_CREDENTIALS_PATH: &str = "credentials.toml";
pub const DEFAULT_RULES_PATH: &str = "rules.toml";

pub struct Built {
    pub settings: Settings,
    pub credentials_path: String,
    pub rules_path: String,
}

pub fn build() -> Built {
    let builder = Settings::builder()
        .listen_address(
            crate::get_predefined_params()
                .listen_address
                .clone()
                .unwrap_or_else(|| {
                    ask_for_input(
                        &format!(
                            "{} (native: 0.0.0.0:443; Docker with 443:8443 mapping: 0.0.0.0:8443)",
                            Settings::doc_listen_address()
                        ),
                        Some(Settings::default_listen_address().to_string()),
                    )
                }),
        )
        .unwrap();

    // Collect credentials first, then build settings
    let (credentials_path, clients) = build_credentials();

    Built {
        settings: builder
            .listen_protocols(ListenProtocolSettings {
                http1: Some(Http1Settings::builder().build()),
                http2: Some(Http2Settings::builder().build()),
                quic: Some(QuicSettings::builder().build()),
            })
            .clients(clients)
            .build()
            .expect("Couldn't build the library settings"),
        credentials_path,
        rules_path: build_rules(),
    }
}

fn build_credentials() -> (String, Vec<Client>) {
    if crate::get_mode() != Mode::NonInteractive
        && check_file_exists(".", DEFAULT_CREDENTIALS_PATH)
        && ask_for_agreement(&format!(
            "Reuse the existing credentials file: {DEFAULT_CREDENTIALS_PATH}?"
        ))
    {
        let clients = read_credentials_file(DEFAULT_CREDENTIALS_PATH).unwrap_or_default();
        return (DEFAULT_CREDENTIALS_PATH.into(), clients);
    }

    let path = ask_for_input::<String>(
        "Path to the credentials file",
        Some(DEFAULT_CREDENTIALS_PATH.into()),
    );

    let users = build_user_list();

    if checked_overwrite(&path, "Overwrite the existing credentials file?") {
        fs::write(&path, compose_credentials_content(users.iter().cloned()))
            .expect("Couldn't write the credentials into a file");
        println!("The user credentials are written to the file: {}", path);
    }

    let clients = users
        .into_iter()
        .map(|(username, password)| Client {
            username,
            password,
            max_http2_conns: None,
            max_http3_conns: None,
        })
        .collect();

    (path, clients)
}

fn read_credentials_file(path: &str) -> Option<Vec<Client>> {
    let content = fs::read_to_string(path).ok()?;
    let doc: toml_edit::Document = content.parse().ok()?;
    let tables = doc.get("client")?.as_array_of_tables()?;
    Some(
        tables
            .iter()
            .filter_map(|t| {
                Some(Client {
                    username: t.get("username")?.as_str()?.to_string(),
                    password: t.get("password")?.as_str()?.to_string(),
                    max_http2_conns: t
                        .get("max_http2_conns")
                        .and_then(|v| v.as_integer())
                        .map(|v| v as u32),
                    max_http3_conns: t
                        .get("max_http3_conns")
                        .and_then(|v| v.as_integer())
                        .map(|v| v as u32),
                })
            })
            .collect(),
    )
}

fn build_rules() -> String {
    if crate::get_mode() != Mode::NonInteractive
        && check_file_exists(".", DEFAULT_RULES_PATH)
        && ask_for_agreement(&format!(
            "Reuse the existing rules file: {DEFAULT_RULES_PATH}?"
        ))
    {
        DEFAULT_RULES_PATH.into()
    } else {
        let path =
            ask_for_input::<String>("Path to the rules file", Some(DEFAULT_RULES_PATH.into()));

        if checked_overwrite(&path, "Overwrite the existing rules file?") {
            println!("Let's create connection filtering rules");
            let rules_config = crate::rules_settings::build();
            let rules_content = generate_rules_toml_content(&rules_config);
            fs::write(&path, rules_content).expect("Couldn't write the rules into a file");
            println!("The rules configuration is written to the file: {}", path);
        }

        path
    }
}

fn build_user_list() -> Vec<(String, String)> {
    if let Some(x) = crate::get_predefined_params().credentials.clone() {
        return vec![x];
    }

    let mut list = vec![(
        ask_for_input::<String>("Username", None),
        ask_for_password("Password"),
    )];

    loop {
        if "no" == select_variant("Add one more user?", &["yes", "no"], Some(1)) {
            break;
        }

        list.push((
            ask_for_input::<String>("Username", None),
            ask_for_password("Password"),
        ));
    }

    list
}

fn compose_credentials_content(clients: impl Iterator<Item = (String, String)>) -> String {
    let mut doc = toml_edit::Document::new();

    let x = clients
        .map(|(u, p)| {
            Table::from_iter(
                std::iter::once(("username", u)).chain(std::iter::once(("password", p))),
            )
        })
        .collect::<ArrayOfTables>();

    doc.insert_formatted(&Key::new("client"), Item::ArrayOfTables(x));

    doc.to_string()
}

fn generate_rules_toml_content(rules_config: &trusttunnel::rules::RulesConfig) -> String {
    let mut content = String::new();

    content.push_str("# Rules configuration for VPN endpoint connection filtering\n");
    content.push_str("#\n");
    content.push_str("# Rules are split into two independent sections:\n");
    content.push_str("#   [inbound]  - Client filtering (evaluated at TLS handshake)\n");
    content.push_str("#   [outbound] - Destination filtering (evaluated per request)\n");
    content.push_str("#\n");
    content.push_str("# Each section has its own default_action and rules list.\n");
    content.push_str("# Rules are evaluated in order; first match wins.\n");
    content.push_str("# If no rules match, default_action is used (\"allow\" if not set).\n");
    content.push_str("#\n");
    content.push_str("# Inbound rule fields:\n");
    content.push_str("#   cidr - IP address range in CIDR notation\n");
    content.push_str("#   client_random_prefix - Hex-encoded TLS client random prefix\n");
    content.push_str("#     Simple: \"aabbcc\" (prefix matching)\n");
    content
        .push_str("#     Masked: \"a0b0/f0f0\" (bitwise: client_random & mask == prefix & mask)\n");
    content.push_str("#   action - \"allow\" or \"deny\"\n");
    content.push_str("#\n");
    content.push_str("# Outbound rule fields:\n");
    content
        .push_str("#   destination_port - Port or port range (e.g., \"6881\" or \"6881-6889\")\n");
    content.push_str("#   destination_cidr - IP range in CIDR notation (e.g., \"10.0.0.0/8\")\n");
    content.push_str("#   action - \"allow\" or \"deny\"\n\n");

    // [inbound] section
    content.push_str("[inbound]\n");
    if let Some(ref action) = rules_config.inbound.default_action {
        content.push_str(&format!(
            "default_action = \"{}\"\n",
            match action {
                trusttunnel::rules::RuleAction::Allow => "allow",
                trusttunnel::rules::RuleAction::Deny => "deny",
            }
        ));
    } else {
        content.push_str("# default_action = \"allow\"\n");
    }
    content.push('\n');

    for rule in &rules_config.inbound.rule {
        content.push_str("[[inbound.rule]]\n");
        if let Some(ref cidr) = rule.cidr {
            content.push_str(&format!("cidr = \"{}\"\n", cidr));
        }
        if let Some(ref prefix) = rule.client_random_prefix {
            content.push_str(&format!("client_random_prefix = \"{}\"\n", prefix));
        }
        content.push_str(&format!(
            "action = \"{}\"\n\n",
            match rule.action {
                trusttunnel::rules::RuleAction::Allow => "allow",
                trusttunnel::rules::RuleAction::Deny => "deny",
            }
        ));
    }

    // [outbound] section
    content.push_str("[outbound]\n");
    if let Some(ref action) = rules_config.outbound.default_action {
        content.push_str(&format!(
            "default_action = \"{}\"\n",
            match action {
                trusttunnel::rules::RuleAction::Allow => "allow",
                trusttunnel::rules::RuleAction::Deny => "deny",
            }
        ));
    } else {
        content.push_str("# default_action = \"allow\"\n");
    }
    content.push('\n');

    for rule in &rules_config.outbound.rule {
        content.push_str("[[outbound.rule]]\n");
        if let Some(ref port) = rule.destination_port {
            content.push_str(&format!("destination_port = \"{}\"\n", port));
        }
        if let Some(cidr) = rule.destination_cidr {
            content.push_str(&format!("destination_cidr = \"{}\"\n", cidr));
        }
        content.push_str(&format!(
            "action = \"{}\"\n\n",
            match rule.action {
                trusttunnel::rules::RuleAction::Allow => "allow",
                trusttunnel::rules::RuleAction::Deny => "deny",
            }
        ));
    }

    content
}

fn check_file_exists(path: &str, name: &str) -> bool {
    match fs::read_dir(path) {
        Ok(x) => x
            .filter_map(Result::ok)
            .filter(|entry| {
                entry
                    .metadata()
                    .map(|meta| meta.is_file())
                    .unwrap_or_default()
            })
            .any(|entry| Ok(name) == entry.file_name().into_string().as_ref().map(String::as_str)),
        Err(_) => false,
    }
}
