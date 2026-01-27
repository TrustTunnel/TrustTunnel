use crate::user_interaction::{
    ask_for_agreement, ask_for_input, ask_for_password, checked_overwrite, select_variant,
};
use crate::Mode;
use std::fs;
use ring::rand::{SecureRandom, SystemRandom};
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
    let mut builder = Settings::builder()
        .listen_address(
            crate::get_predefined_params()
                .listen_address
                .clone()
                .unwrap_or_else(|| {
                    ask_for_input(
                        &format!(
                            "{} (use 0.0.0.0:443 for all interfaces on HTTPS port)",
                            Settings::doc_listen_address()
                        ),
                        Some(Settings::default_listen_address().to_string()),
                    )
                }),
        )
        .unwrap();

    let allow_without_token =
        select_variant("Allow tunnel without token? (legacy)", &["no", "yes"], Some(0)) == "yes";
    builder = builder.allow_without_token(allow_without_token);

    let ping_enable = select_variant("Enable ping check?", &["yes", "no"], Some(0)) == "yes";
    if ping_enable {
        let ping_path_default = default_path("ping");
        let ping_path = ask_for_input::<String>(
            "Ping check path (default: random)",
            Some(ping_path_default),
        );
        builder = builder.ping_enable(true).ping_path(ping_path);
    } else {
        builder = builder.ping_enable(false);
    }

    let speedtest_enable =
        select_variant("Enable speedtest?", &["yes", "no"], Some(1)) == "yes";
    if speedtest_enable {
        let speedtest_path_default = default_path("speedtest");
        let speedtest_path = ask_for_input::<String>(
            "Speedtest path (default: random)",
            Some(speedtest_path_default),
        );
        builder = builder.speedtest_enable(true).speedtest_path(speedtest_path);
    }

    // Collect credentials last, then build settings
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

fn random_token() -> String {
    let mut bytes = [0u8; 16];
    SystemRandom::new().fill(&mut bytes).unwrap();
    hex::encode(bytes)
}

fn default_path(prefix: &str) -> String {
    format!("/{prefix}-{}", random_token())
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
        .map(|(username, password)| Client { username, password })
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

    // Add header comments explaining the format
    content.push_str("# Rules configuration for VPN endpoint connection filtering\n");
    content.push_str("# \n");
    content.push_str("# This file defines filter rules for incoming connections.\n");
    content.push_str(
        "# Rules are evaluated in order, and the first matching rule's action is applied.\n",
    );
    content.push_str("# If no rules match, the connection is allowed by default.\n");
    content.push_str("#\n");
    content.push_str("# Each rule can specify:\n");
    content.push_str("# - cidr: IP address range in CIDR notation\n");
    content.push_str("# - client_random_prefix: Hex-encoded prefix of TLS client random data\n");
    content.push_str(
        "#   Can optionally include a mask in format \"prefix[/mask]\" for bitwise matching\n",
    );
    content.push_str("# - action: \"allow\" or \"deny\"\n");
    content.push_str("#\n");
    content.push_str("# All fields except 'action' are optional - if specified, all conditions must match for the rule to apply.\n");
    content.push_str("#\n");
    content.push_str("# client_random_prefix formats:\n");
    content.push_str("# 1. Simple prefix matching:\n");
    content.push_str("#    client_random_prefix = \"aabbcc\"\n");
    content.push_str("#    → matches client_random starting with 0xaabbcc\n");
    content.push_str("#\n");
    content.push_str("# 2. Bitwise matching with mask:\n");
    content.push_str("#    client_random_prefix = \"a0b0/f0f0\"\n");
    content.push_str("#    → prefix=a0b0, mask=f0f0\n");
    content.push_str(
        "#    → matches client_random where (client_random & 0xf0f0) == (0xa0b0 & 0xf0f0)\n",
    );
    content.push_str("#    → e.g., 0xa5b5, 0xa9bf match, but 0xb0b0, 0xa0c0 don't match\n\n");

    // Serialize the actual rules (usually empty)
    if !rules_config.rule.is_empty() {
        content.push_str(&toml::ser::to_string(rules_config).unwrap());
        content.push('\n');
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
