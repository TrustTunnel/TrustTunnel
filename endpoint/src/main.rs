use log::{debug, error, info, warn, LevelFilter};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::Arc;
use tokio::signal;
use toml_edit::{Document, Item, Table};
use trusttunnel::authentication::registry_based::RegistryBasedAuthenticator;
use trusttunnel::authentication::Authenticator;
use trusttunnel::client_config;
use trusttunnel::client_random_prefix::{self, GeneratorParams};
use trusttunnel::core::Core;
use trusttunnel::settings::Settings;
use trusttunnel::shutdown::Shutdown;
use trusttunnel::{log_utils, settings};

const VERSION_STRING: &str = env!("TRUSTTUNNEL_VERSION");
const VERSION_PARAM_NAME: &str = "v_e_r_s_i_o_n_do_not_change_this_name_it_will_break";
const LOG_LEVEL_PARAM_NAME: &str = "log_level";
const LOG_FILE_PARAM_NAME: &str = "log_file";
const SETTINGS_PARAM_NAME: &str = "settings";
const TLS_HOSTS_SETTINGS_PARAM_NAME: &str = "tls_hosts_settings";
const CLIENT_CONFIG_PARAM_NAME: &str = "client_config";
const ADDRESS_PARAM_NAME: &str = "address";
const CUSTOM_SNI_PARAM_NAME: &str = "custom_sni";
const CLIENT_RANDOM_PREFIX_PARAM_NAME: &str = "client_random_prefix";
const CLIENT_RANDOM_PSK_KEY_PARAM_NAME: &str = "client_random_psk_key";
const GENERATE_CLIENT_RANDOM_PREFIX_PARAM_NAME: &str = "generate_client_random_prefix";
const PREFIX_LENGTH_PARAM_NAME: &str = "prefix_length";
const PREFIX_PERCENT_PARAM_NAME: &str = "prefix_percent";
const PREFIX_MASK_PARAM_NAME: &str = "prefix_mask";
const FORMAT_PARAM_NAME: &str = "format";
const NAME_PARAM_NAME: &str = "name";
const DNS_UPSTREAM_PARAM_NAME: &str = "dns_upstream";
const SENTRY_DSN_PARAM_NAME: &str = "sentry_dsn";
const THREADS_NUM_PARAM_NAME: &str = "threads_num";
const TRUSTTUNNEL_QR_URL: &str = "https://trusttunnel.org/qr.html";

#[cfg(unix)]
fn increase_fd_limit() {
    use nix::sys::resource::{getrlimit, setrlimit, Resource};
    let max_rlim = 65536;

    let (soft, hard) = match getrlimit(Resource::RLIMIT_NOFILE) {
        Ok(limits) => limits,
        Err(err) => {
            warn!("Failed to get file descriptor limit: {}", err);
            return;
        }
    };

    let target_limit = std::cmp::min(hard, max_rlim);
    if soft >= target_limit {
        debug!(
            "File descriptor limit is already {} (target: {})",
            soft, target_limit
        );
        return;
    }

    if let Err(err) = setrlimit(Resource::RLIMIT_NOFILE, target_limit, hard) {
        warn!(
            "Failed to increase file descriptor limit from {} to {}: {}",
            soft, target_limit, err
        );
        return;
    }

    debug!(
        "Successfully increased file descriptor limit to {}",
        target_limit
    );
}

#[cfg(not(unix))]
fn increase_fd_limit() {}

fn main() {
    let args = clap::Command::new("VPN endpoint")
        .args(&[
            // Built-in version parameter handling is deficient in that it
            // outputs `<program name> <version>` instead of just `<version>`
            // and also uses `-V` instead of `-v` as the shorthand.
            clap::Arg::new(VERSION_PARAM_NAME)
                .short('v')
                .long("version")
                .action(clap::ArgAction::SetTrue)
                .help("Print the version of this software and exit"),
            clap::Arg::new(LOG_LEVEL_PARAM_NAME)
                .short('l')
                .long("loglvl")
                .action(clap::ArgAction::Set)
                .value_parser(["info", "debug", "trace"])
                .default_value("info")
                .help("Logging level"),
            clap::Arg::new(LOG_FILE_PARAM_NAME)
                .long("logfile")
                .action(clap::ArgAction::Set)
                .help("File path for storing logs. If not specified, the logs are printed to stdout"),
            clap::Arg::new(SENTRY_DSN_PARAM_NAME)
                .long(SENTRY_DSN_PARAM_NAME)
                .action(clap::ArgAction::Set)
                .help("Sentry DSN (see https://docs.sentry.io/product/sentry-basics/dsn-explainer/ for details)"),
            clap::Arg::new(THREADS_NUM_PARAM_NAME)
                .long("jobs")
                .action(clap::ArgAction::Set)
                .value_parser(clap::value_parser!(usize))
                .help("The number of worker threads. If not specified, set to the number of CPUs on the machine."),
            clap::Arg::new(SETTINGS_PARAM_NAME)
                .action(clap::ArgAction::Set)
                .required_unless_present(VERSION_PARAM_NAME)
                .help("Path to a settings file"),
            clap::Arg::new(TLS_HOSTS_SETTINGS_PARAM_NAME)
                .action(clap::ArgAction::Set)
                .required_unless_present(VERSION_PARAM_NAME)
                .help("Path to a file containing TLS hosts settings. Sending SIGHUP to the process causes reloading the settings."),
            clap::Arg::new(CLIENT_CONFIG_PARAM_NAME)
                .action(clap::ArgAction::Set)
                .requires(ADDRESS_PARAM_NAME)
                .short('c')
                .long("client_config")
                .value_names(["client_name"])
                .help("Print the endpoint config for specified client and exit."),
            clap::Arg::new(ADDRESS_PARAM_NAME)
                .action(clap::ArgAction::Append)
                .requires(CLIENT_CONFIG_PARAM_NAME)
                .short('a')
                .long("address")
                .help("Endpoint address to be added to client's config. Accepts ip, ip:port, domain, or domain:port."),
            clap::Arg::new(CUSTOM_SNI_PARAM_NAME)
                .action(clap::ArgAction::Set)
                .requires(CLIENT_CONFIG_PARAM_NAME)
                .short('s')
                .long("custom-sni")
                .help("Custom SNI override for client connection. Must match an allowed_sni in hosts.toml."),
            clap::Arg::new(CLIENT_RANDOM_PREFIX_PARAM_NAME)
                .action(clap::ArgAction::Set)
                .requires(CLIENT_CONFIG_PARAM_NAME)
                .short('r')
                .long("client-random-prefix")
                .help("TLS client random hex prefix for connection filtering. Must have a corresponding rule in rules.toml."),
            clap::Arg::new(CLIENT_RANDOM_PSK_KEY_PARAM_NAME)
                .action(clap::ArgAction::Set)
                .requires(CLIENT_CONFIG_PARAM_NAME)
                .long("client-random-psk-key")
                .conflicts_with(CLIENT_RANDOM_PREFIX_PARAM_NAME)
                .conflicts_with(GENERATE_CLIENT_RANDOM_PREFIX_PARAM_NAME)
                .help("TLS client random PSK key (hex). Derives full client_random via HKDF+AES. Must have a corresponding rule with client_random_psk_key in rules.toml."),
            clap::Arg::new(GENERATE_CLIENT_RANDOM_PREFIX_PARAM_NAME)
                .action(clap::ArgAction::SetTrue)
                .requires(CLIENT_CONFIG_PARAM_NAME)
                .long("generate-client-random-prefix")
                .conflicts_with(CLIENT_RANDOM_PREFIX_PARAM_NAME)
                .help("Generate a new TLS client random prefix for connection filtering and use it in the exported client config."),
            clap::Arg::new(PREFIX_LENGTH_PARAM_NAME)
                .action(clap::ArgAction::Set)
                .requires(GENERATE_CLIENT_RANDOM_PREFIX_PARAM_NAME)
                .long("prefix-length")
                .value_parser(clap::value_parser!(usize))
                .default_value("4")
                .help("Generated client random prefix length in bytes."),
            clap::Arg::new(PREFIX_PERCENT_PARAM_NAME)
                .action(clap::ArgAction::Set)
                .requires(GENERATE_CLIENT_RANDOM_PREFIX_PARAM_NAME)
                .long("prefix-percent")
                .value_parser(clap::value_parser!(u8))
                .default_value("70")
                .help("Percentage of one bits in the generated client random mask."),
            clap::Arg::new(PREFIX_MASK_PARAM_NAME)
                .action(clap::ArgAction::Set)
                .requires(GENERATE_CLIENT_RANDOM_PREFIX_PARAM_NAME)
                .long("prefix-mask")
                .conflicts_with_all([PREFIX_LENGTH_PARAM_NAME, PREFIX_PERCENT_PARAM_NAME])
                .help("Explicit mask for generated client random prefix in hex format."),
            clap::Arg::new(FORMAT_PARAM_NAME)
                .action(clap::ArgAction::Set)
                .requires(CLIENT_CONFIG_PARAM_NAME)
                .short('f')
                .long("format")
                .value_parser(["toml", "deeplink"])
                .default_value("deeplink")
                .help("Output format for client configuration: 'deeplink' produces tt://? URI, 'toml' produces traditional config file"),
            clap::Arg::new(NAME_PARAM_NAME)
                .action(clap::ArgAction::Set)
                .requires(CLIENT_CONFIG_PARAM_NAME)
                .short('n')
                .long("name")
                .help("Human-readable server display name for the client configuration."),
            clap::Arg::new(DNS_UPSTREAM_PARAM_NAME)
                .action(clap::ArgAction::Append)
                .requires(CLIENT_CONFIG_PARAM_NAME)
                .short('d')
                .long("dns-upstream")
                .help("DNS upstream address to include in the client configuration. Can be specified multiple times."),
        ])
        .disable_version_flag(true)
        .get_matches();

    if args.contains_id(VERSION_PARAM_NAME)
        && Some(true) == args.get_one::<bool>(VERSION_PARAM_NAME).copied()
    {
        println!("{}", VERSION_STRING);
        return;
    }

    #[cfg(feature = "tracing")]
    console_subscriber::init();

    let _guard = args.get_one::<String>(SENTRY_DSN_PARAM_NAME).map(|x| {
        sentry::init((
            x.clone(),
            sentry::ClientOptions {
                release: sentry::release_name!(),
                ..Default::default()
            },
        ))
    });

    let _guard = log_utils::LogFlushGuard;
    log::set_logger(match args.get_one::<String>(LOG_FILE_PARAM_NAME) {
        None => log_utils::make_stdout_logger(),
        Some(file) => log_utils::make_file_logger(file).expect("Couldn't open the logging file"),
    })
    .expect("Couldn't set logger");

    log::set_max_level(
        match args
            .get_one::<String>(LOG_LEVEL_PARAM_NAME)
            .map(String::as_str)
        {
            None => LevelFilter::Info,
            Some("info") => LevelFilter::Info,
            Some("debug") => LevelFilter::Debug,
            Some("trace") => LevelFilter::Trace,
            Some(x) => panic!("Unexpected log level: {}", x),
        },
    );

    increase_fd_limit();

    let settings_path = args.get_one::<String>(SETTINGS_PARAM_NAME).unwrap();
    let settings_contents =
        std::fs::read_to_string(settings_path).expect("Couldn't read the settings file");
    let settings: Settings =
        toml::from_str(&settings_contents).expect("Couldn't parse the settings file");

    if settings.get_clients().is_empty() && settings.get_listen_address().ip().is_loopback() {
        warn!(
            "No credentials configured (credentials_file is missing). \
            Anyone can connect to this endpoint. This is acceptable for local development \
            but should not be used in production."
        );
    }

    let tls_hosts_settings_path = args
        .get_one::<String>(TLS_HOSTS_SETTINGS_PARAM_NAME)
        .unwrap();
    let tls_hosts_settings: settings::TlsHostsSettings = toml::from_str(
        &std::fs::read_to_string(tls_hosts_settings_path)
            .expect("Couldn't read the TLS hosts settings file"),
    )
    .expect("Couldn't parse the TLS hosts settings file");

    if args.contains_id(CLIENT_CONFIG_PARAM_NAME) {
        let username = args.get_one::<String>(CLIENT_CONFIG_PARAM_NAME).unwrap();
        let listen_port = settings.get_listen_address().port();
        let addresses: Vec<String> = args
            .get_many::<String>(ADDRESS_PARAM_NAME)
            .expect("At least one address should be specified")
            .map(|x| parse_endpoint_address(x, listen_port))
            .collect();

        for addr in &addresses {
            if let Some(domain) = extract_domain_for_warning(addr) {
                if !domain_matches_tls_hosts(domain, &tls_hosts_settings) {
                    warn!(
                        "Domain '{}' does not match any hostname in TLS hosts settings. \
                         Please verify this is correct (it may be a typo).",
                        domain
                    );
                }
            }
        }

        let custom_sni = args.get_one::<String>(CUSTOM_SNI_PARAM_NAME).cloned();
        if let Some(ref sni) = custom_sni {
            let is_valid = tls_hosts_settings
                .get_main_hosts()
                .iter()
                .any(|host| host.hostname == *sni || host.allowed_sni.contains(sni));
            if !is_valid {
                eprintln!(
                    "Error: custom SNI '{}' does not match any hostname or allowed_sni in hosts.toml",
                    sni
                );
                std::process::exit(1);
            }
        }

        let generated_client_random_prefix = if args
            .get_flag(GENERATE_CLIENT_RANDOM_PREFIX_PARAM_NAME)
        {
            let generated =
                if let Some(mask_hex) = args.get_one::<String>(PREFIX_MASK_PARAM_NAME) {
                    let mask = hex::decode(mask_hex).unwrap_or_else(|_| {
                        eprintln!("Error: prefix_mask '{}' is not valid hex", mask_hex);
                        std::process::exit(1);
                    });

                    client_random_prefix::generate_with_mask(mask)
                } else {
                    client_random_prefix::generate(GeneratorParams {
                        length: *args.get_one::<usize>(PREFIX_LENGTH_PARAM_NAME).unwrap(),
                        percent: *args.get_one::<u8>(PREFIX_PERCENT_PARAM_NAME).unwrap(),
                    })
                }
                .unwrap_or_else(|err| {
                    eprintln!("Error: {}", err);
                    std::process::exit(1);
                });

            let generated_prefix = generated.to_masked_hex_string();
            let rules_path = extract_rules_file_path(&settings_contents, settings_path).unwrap_or_else(|| {
                eprintln!(
                    "Error: rules_file must be configured in settings to generate client_random_prefix"
                );
                std::process::exit(1);
            });

            append_allow_rule(&rules_path, &generated_prefix).unwrap_or_else(|err| {
                eprintln!(
                    "Error: failed to append generated client_random_prefix to '{}': {}",
                    rules_path.display(),
                    err
                );
                std::process::exit(1);
            });

            eprintln!(
                "Added allow rule to '{}': {}",
                rules_path.display(),
                generated_prefix
            );

            Some(generated_prefix)
        } else {
            None
        };

        let is_generated = generated_client_random_prefix.is_some();
        let mut client_random_prefix = generated_client_random_prefix.or_else(|| {
            args.get_one::<String>(CLIENT_RANDOM_PREFIX_PARAM_NAME)
                .cloned()
        });

        // Validate explicit --client-random-prefix (skip for generated prefix)
        if !is_generated {
            if let Some(ref prefix) = client_random_prefix {
                let has_slash = prefix.contains('/');
                let (input_prefix, input_mask) = prefix.split_once('/').unwrap_or((prefix, ""));

                // Validate hex format
                if hex::decode(input_prefix).is_err() {
                    eprintln!("Error: client_random_prefix '{}' is not valid hex", prefix);
                    std::process::exit(1);
                }

                if (has_slash && input_mask.is_empty())
                    || (!input_mask.is_empty() && hex::decode(input_mask).is_err())
                {
                    eprintln!(
                        "Error: client_random_prefix mask '{}' is not valid hex",
                        input_mask
                    );
                    std::process::exit(1);
                }

                // Validate against rules.toml
                if let Some(rules_engine) = settings.get_rules_engine() {
                    let input_mask: Option<&str> = if input_mask.is_empty() {
                        None
                    } else {
                        Some(input_mask)
                    };

                    let matching_rule = rules_engine.config().rule.iter().find(|rule| {
                        rule.client_random_prefix
                            .as_ref()
                            .map(|p| {
                                let (rule_prefix, rule_mask): (&str, Option<&str>) = p
                                    .split_once('/')
                                    .map(|(a, b)| (a, Some(b)))
                                    .unwrap_or((p.as_str(), None));

                                // Prefix parts must be equal
                                if rule_prefix != input_prefix {
                                    return false;
                                }

                                // Mask compatibility: input mask must be same or stronger than rule mask.
                                // "Stronger" means more bits set, i.e. (input_mask & rule_mask) == rule_mask.
                                match (input_mask, rule_mask) {
                                    // Rule has no mask, any input mask is at least as strong
                                    (_, None) => true,
                                    // Input has no mask, strongest possible
                                    (None, Some(_)) => true,
                                    // Both have masks, input mask must cover all bits of rule mask
                                    (Some(mi_str), Some(mr_str)) => {
                                        match (hex::decode(mi_str), hex::decode(mr_str)) {
                                            (Ok(mi), Ok(mr)) => {
                                                mi.len() >= mr.len()
                                                    && (0..mr.len()).all(|i| mi[i] & mr[i] == mr[i])
                                            }
                                            _ => false,
                                        }
                                    }
                                }
                            })
                            .unwrap_or(false)
                    });

                    // Print warning and continue, do not panic because it's optional field
                    match matching_rule {
                        None => {
                            eprintln!(
                            "Warning: No rule found in rules.toml matching client_random_prefix '{}'. This field will be ignored.",
                            prefix
                        );
                            client_random_prefix = None;
                        }
                        Some(rule) if rule.action == trusttunnel::rules::RuleAction::Deny => {
                            eprintln!(
                            "Warning: Matched rule in rules.toml for client_random_prefix '{}' has action 'deny'.",
                            prefix
                        );
                        }
                        Some(_) => {}
                    }
                }
            }
        }

        let name = args.get_one::<String>(NAME_PARAM_NAME).cloned();
        let dns_upstreams: Vec<String> = args
            .get_many::<String>(DNS_UPSTREAM_PARAM_NAME)
            .map(|vals| vals.cloned().collect())
            .unwrap_or_default();

        let mut client_random_psk_key = args
            .get_one::<String>(CLIENT_RANDOM_PSK_KEY_PARAM_NAME)
            .cloned();

        if let Some(ref psk_key) = client_random_psk_key {
            if !trusttunnel::rules::is_valid_hex(psk_key) {
                // The PSK key is a secret, do not log its value
                eprintln!("Error: client_random_psk_key is not valid hex");
                std::process::exit(1);
            }
            // Validate against rules.toml
            if let Some(rules_engine) = settings.get_rules_engine() {
                // Both operands (CLI arg and rules.toml) are local, so timing is not
                // attacker-controlled and a plain string comparison is acceptable here.
                let matching_rule = rules_engine
                    .config()
                    .rule
                    .iter()
                    .find(|r| r.client_random_psk_key.as_deref() == Some(psk_key.as_str()));
                match matching_rule {
                    None => {
                        // The PSK key is a secret, do not log its value
                        eprintln!("Warning: No rule found in rules.toml with matching client_random_psk_key. This field will be ignored.");
                        client_random_psk_key = None;
                    }
                    Some(rule) if rule.action == trusttunnel::rules::RuleAction::Deny => {
                        eprintln!("Warning: Matched rule in rules.toml for client_random_psk_key has action 'deny'.");
                    }
                    Some(_) => {}
                }
            }
        }

        let client_config = client_config::build(
            username,
            addresses,
            settings.get_clients(),
            &tls_hosts_settings,
            custom_sni,
            client_random_prefix,
            client_random_psk_key,
            name,
            dns_upstreams,
        );

        let format = args
            .get_one::<String>(FORMAT_PARAM_NAME)
            .map(String::as_str)
            .unwrap_or("deeplink");

        match format {
            "toml" => {
                println!("{}", client_config.compose_toml());
            }
            "deeplink" => match client_config.compose_deeplink() {
                Ok(deep_link) => {
                    println!("{deep_link}");
                    println!(
                        "\nTo connect on mobile, you can scan QR code on the page: {TRUSTTUNNEL_QR_URL}#tt={}",
                        deep_link.strip_prefix("tt://?").unwrap()
                    );
                }
                Err(e) => {
                    eprintln!("Error generating deep-link: {}", e);
                    std::process::exit(1);
                }
            },
            _ => {
                eprintln!(
                    "Error: unsupported format '{}'. Use 'toml' or 'deeplink'.",
                    format
                );
                std::process::exit(1);
            }
        }

        return;
    }

    let rt = {
        let mut builder = tokio::runtime::Builder::new_multi_thread();
        builder.enable_io();
        builder.enable_time();

        if let Some(n) = args.get_one::<usize>(THREADS_NUM_PARAM_NAME) {
            builder.worker_threads(*n);
        }

        builder.build().expect("Failed to set up runtime")
    };

    let shutdown = Shutdown::new();
    let authenticator: Option<Arc<dyn Authenticator>> = if !settings.get_clients().is_empty() {
        Some(Arc::new(RegistryBasedAuthenticator::new(
            settings.get_clients(),
        )))
    } else {
        None
    };
    let core = Arc::new(
        Core::new(
            settings,
            authenticator,
            tls_hosts_settings,
            shutdown.clone(),
        )
        .expect("Couldn't create core instance"),
    );

    let listen_task = {
        let core = core.clone();
        async move { core.listen().await }
    };

    let reload_tls_hosts_task = {
        let tls_hosts_settings_path = tls_hosts_settings_path.clone();
        async move {
            let mut sighup_listener = signal::unix::signal(signal::unix::SignalKind::hangup())
                .expect("Couldn't start SIGHUP listener");

            loop {
                sighup_listener.recv().await;
                info!("Reloading TLS hosts settings");

                let tls_hosts_settings: settings::TlsHostsSettings = toml::from_str(
                    &std::fs::read_to_string(&tls_hosts_settings_path)
                        .expect("Couldn't read the TLS hosts settings file"),
                )
                .expect("Couldn't parse the TLS hosts settings file");

                core.reload_tls_hosts_settings(tls_hosts_settings)
                    .expect("Couldn't apply new settings");
                info!("TLS hosts settings are successfully reloaded");
            }
        }
    };

    #[allow(clippy::await_holding_lock)]
    let interrupt_task = async move {
        tokio::signal::ctrl_c().await.unwrap();
        shutdown.lock().unwrap().submit();
        shutdown.lock().unwrap().completion().await
    };

    let exit_code = rt.block_on(async move {
        tokio::select! {
            listen_result = listen_task => match listen_result {
                Ok(()) => 0,
                Err(e) => {
                    error!("Error while listening IO events: {}", e);
                    1
                }
            },
            _ = reload_tls_hosts_task => {
                error!("Error while reloading TLS hosts");
                1
            },
            _ = interrupt_task => {
                info!("Interrupted by user");
                0
            },
        }
    });

    std::process::exit(exit_code);
}

/// Returns the domain part of an address string if it is a domain (not an IP).
/// Returns `None` for IP addresses (both IPv4 and IPv6).
fn extract_domain_for_warning(addr: &str) -> Option<&str> {
    if SocketAddr::from_str(addr).is_ok() {
        return None;
    }
    if addr.parse::<std::net::IpAddr>().is_ok() {
        return None;
    }
    let domain = addr.rsplit_once(':').map(|(d, _)| d).unwrap_or(addr);
    if domain.parse::<std::net::IpAddr>().is_ok() {
        return None;
    }
    Some(domain)
}

fn domain_matches_tls_hosts(domain: &str, tls_hosts_settings: &settings::TlsHostsSettings) -> bool {
    tls_hosts_settings
        .get_main_hosts()
        .iter()
        .any(|h| h.hostname == domain || h.allowed_sni.iter().any(|s| s == domain))
}

fn append_allow_rule(rules_path: &Path, client_random_prefix: &str) -> std::io::Result<()> {
    let content = std::fs::read_to_string(rules_path).unwrap_or_default();
    let mut doc: Document = content
        .parse()
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;

    let mut new_rule = Table::new();
    new_rule.insert(
        "client_random_prefix",
        toml_edit::value(client_random_prefix),
    );
    new_rule.insert("action", toml_edit::value("allow"));

    let rules = doc
        .entry("rule")
        .or_insert(Item::ArrayOfTables(Default::default()))
        .as_array_of_tables_mut()
        .ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::InvalidData, "invalid rules format")
        })?;

    let tail: Vec<Table> = rules.iter().cloned().collect();
    rules.clear();
    rules.push(new_rule);
    for table in tail {
        rules.push(table);
    }

    std::fs::write(rules_path, doc.to_string())
}

fn extract_rules_file_path(settings_contents: &str, settings_path: &str) -> Option<PathBuf> {
    let value = settings_contents.parse::<toml::Value>().ok()?;
    let rules_file = value.get("rules_file")?.as_str()?;
    let path = Path::new(rules_file);

    if path.is_absolute() {
        return Some(path.to_path_buf());
    }

    let settings_dir = Path::new(settings_path)
        .parent()
        .unwrap_or_else(|| Path::new("."));
    Some(settings_dir.join(path))
}

/// Parse an endpoint address string into a normalized `host:port` format.
///
/// Accepts the following formats:
/// - `IP:port` (e.g. `1.2.3.4:443`, `[::1]:443`)
/// - `IP` without port (e.g. `1.2.3.4`, `::1`) — `default_port` is appended
/// - `domain:port` (e.g. `vpn.example.com:443`)
/// - `domain` without port (e.g. `vpn.example.com`) — `default_port` is appended
fn parse_endpoint_address(input: &str, default_port: u16) -> String {
    if let Ok(addr) = SocketAddr::from_str(input) {
        return addr.to_string();
    }
    if let Ok(addr) = SocketAddr::from_str(&format!("{input}:{default_port}")) {
        return addr.to_string();
    }
    if let Ok(ip) = input.parse::<std::net::IpAddr>() {
        return SocketAddr::new(ip, default_port).to_string();
    }
    if let Some((domain, port_str)) = input.rsplit_once(':') {
        let port: u16 = port_str.parse().unwrap_or_else(|_| {
            panic!(
                "Failed to parse port in address '{}'. \
                 Expected `ip`, `ip:port`, `domain`, or `domain:port` format.",
                input
            );
        });
        format!("{domain}:{port}")
    } else {
        format!("{input}:{default_port}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_tls_hosts(hostnames: &[&str]) -> settings::TlsHostsSettings {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let id = COUNTER.fetch_add(1, Ordering::Relaxed);
        let tmp = std::env::temp_dir().join(format!("trusttunnel_test_cert_{id}.pem"));
        std::fs::write(&tmp, b"").unwrap();
        let path = tmp.to_str().unwrap();
        let entries: String = hostnames
            .iter()
            .map(|&h| {
                format!(
                    "[[main_hosts]]\nhostname = \"{}\"\ncert_chain_path = \"{}\"\nprivate_key_path = \"{}\"\n",
                    h, path, path
                )
            })
            .collect();
        toml::from_str(&entries).unwrap()
    }

    #[test]
    fn test_extract_domain_ipv4_returns_none() {
        assert_eq!(extract_domain_for_warning("1.2.3.4:443"), None);
    }

    #[test]
    fn test_extract_domain_ipv6_returns_none() {
        assert_eq!(extract_domain_for_warning("[::1]:443"), None);
    }

    #[test]
    fn test_extract_domain_bare_ipv6_returns_none() {
        assert_eq!(extract_domain_for_warning("::1"), None);
    }

    #[test]
    fn test_extract_domain_with_port_returns_domain() {
        assert_eq!(
            extract_domain_for_warning("vpn.example.com:443"),
            Some("vpn.example.com")
        );
    }

    #[test]
    fn test_extract_domain_without_port_returns_domain() {
        assert_eq!(
            extract_domain_for_warning("vpn.example.com"),
            Some("vpn.example.com")
        );
    }

    #[test]
    fn test_domain_matches_tls_hosts_exact() {
        let hosts = make_tls_hosts(&["vpn.example.com"]);
        assert!(domain_matches_tls_hosts("vpn.example.com", &hosts));
    }

    #[test]
    fn test_domain_matches_tls_hosts_no_match() {
        let hosts = make_tls_hosts(&["vpn.example.com"]);
        assert!(!domain_matches_tls_hosts("other.example.com", &hosts));
    }

    #[test]
    fn test_domain_matches_tls_hosts_allowed_sni() {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(1000);
        let id = COUNTER.fetch_add(1, Ordering::Relaxed);
        let tmp = std::env::temp_dir().join(format!("trusttunnel_test_cert_{id}.pem"));
        std::fs::write(&tmp, b"").unwrap();
        let path = tmp.to_str().unwrap();
        let toml = format!(
            "[[main_hosts]]\nhostname = \"vpn.example.com\"\ncert_chain_path = \"{}\"\nprivate_key_path = \"{}\"\nallowed_sni = [\"alias.example.com\"]\n",
            path, path
        );
        let hosts: settings::TlsHostsSettings = toml::from_str(&toml).unwrap();
        assert!(domain_matches_tls_hosts("alias.example.com", &hosts));
    }

    #[test]
    fn test_domain_matches_tls_hosts_different_host() {
        let hosts = make_tls_hosts(&["other.example.com"]);
        assert!(!domain_matches_tls_hosts("vpn.example.com", &hosts));
    }

    #[test]
    fn test_parse_ipv4_with_port() {
        assert_eq!(parse_endpoint_address("1.2.3.4:443", 8443), "1.2.3.4:443");
    }

    #[test]
    fn test_parse_ipv4_without_port() {
        assert_eq!(parse_endpoint_address("1.2.3.4", 443), "1.2.3.4:443");
    }

    #[test]
    fn test_parse_ipv6_with_port() {
        assert_eq!(parse_endpoint_address("[::1]:443", 8443), "[::1]:443");
    }

    #[test]
    fn test_parse_ipv6_without_port() {
        assert_eq!(parse_endpoint_address("::1", 443), "[::1]:443");
    }

    #[test]
    fn test_parse_domain_with_port() {
        assert_eq!(
            parse_endpoint_address("vpn.example.com:8443", 443),
            "vpn.example.com:8443"
        );
    }

    #[test]
    fn test_parse_domain_without_port() {
        assert_eq!(
            parse_endpoint_address("vpn.example.com", 443),
            "vpn.example.com:443"
        );
    }

    #[test]
    fn test_parse_domain_default_port_applied() {
        assert_eq!(
            parse_endpoint_address("my-vpn.example.org", 8443),
            "my-vpn.example.org:8443"
        );
    }

    #[test]
    #[should_panic(expected = "Failed to parse port")]
    fn test_parse_domain_invalid_port() {
        parse_endpoint_address("vpn.example.com:notaport", 443);
    }

    #[test]
    fn test_parse_ipv6_with_port_bracket_notation() {
        assert_eq!(
            parse_endpoint_address("[2001:db8::1]:443", 8443),
            "[2001:db8::1]:443"
        );
    }

    #[test]
    fn test_parse_bare_ipv6_without_port_gets_default() {
        assert_eq!(
            parse_endpoint_address("2001:db8::1", 443),
            "[2001:db8::1]:443"
        );
    }

    #[test]
    fn test_extract_rules_file_path_resolves_relative_to_settings_file() {
        let settings_dir = std::env::temp_dir().join("trusttunnel_settings_dir_relative");
        std::fs::create_dir_all(&settings_dir).unwrap();
        let settings_path = settings_dir.join("vpn.toml");
        let settings_contents = r#"rules_file = "rules.toml""#;

        let rules_path =
            extract_rules_file_path(settings_contents, settings_path.to_str().unwrap()).unwrap();

        assert_eq!(rules_path, settings_dir.join("rules.toml"));
    }

    #[test]
    fn test_extract_rules_file_path_keeps_absolute_path() {
        let absolute_rules = std::env::temp_dir().join("trusttunnel_absolute_rules.toml");
        let settings_path = std::env::temp_dir().join("trusttunnel_settings.toml");
        let settings_contents = format!("rules_file = \"{}\"", absolute_rules.display());

        let rules_path =
            extract_rules_file_path(&settings_contents, settings_path.to_str().unwrap()).unwrap();

        assert_eq!(rules_path, absolute_rules);
    }

    #[test]
    fn test_extract_rules_file_path_returns_none_without_rules_file() {
        assert!(
            extract_rules_file_path("listen_address = \"127.0.0.1:443\"", "vpn.toml").is_none()
        );
    }

    #[test]
    fn test_append_allow_rule_creates_new_rule() {
        let rules_path = std::env::temp_dir().join("trusttunnel_append_allow_rule_create.toml");
        let _ = std::fs::remove_file(&rules_path);

        append_allow_rule(&rules_path, "abcd/fff0").unwrap();

        let contents = std::fs::read_to_string(&rules_path).unwrap();
        assert!(contents.contains("[[rule]]"));
        assert!(contents.contains("client_random_prefix = \"abcd/fff0\""));
        assert!(contents.contains("action = \"allow\""));

        let _ = std::fs::remove_file(&rules_path);
    }

    #[test]
    fn test_append_allow_rule_appends_after_existing_allow() {
        let rules_path = std::env::temp_dir().join("trusttunnel_append_allow_rule_append.toml");
        std::fs::write(
            &rules_path,
            "[[rule]]\ncidr = \"10.0.0.0/8\"\naction = \"allow\"\n",
        )
        .unwrap();

        append_allow_rule(&rules_path, "1234/ff00").unwrap();

        let contents = std::fs::read_to_string(&rules_path).unwrap();
        assert!(contents.contains("cidr = \"10.0.0.0/8\""));
        assert!(contents.contains("client_random_prefix = \"1234/ff00\""));

        let _ = std::fs::remove_file(&rules_path);
    }

    #[test]
    fn test_append_allow_rule_inserts_before_catchall_deny() {
        let rules_path =
            std::env::temp_dir().join("trusttunnel_append_allow_rule_before_catchall.toml");
        std::fs::write(&rules_path, "[[rule]]\naction = \"deny\"\n").unwrap();

        append_allow_rule(&rules_path, "abcd/fff0").unwrap();

        let contents = std::fs::read_to_string(&rules_path).unwrap();
        let allow_pos = contents.find("client_random_prefix").unwrap();
        let deny_pos = contents.find("action = \"deny\"").unwrap();
        assert!(
            allow_pos < deny_pos,
            "allow rule should appear before catch-all deny"
        );

        let _ = std::fs::remove_file(&rules_path);
    }

    #[test]
    fn test_append_allow_rule_inserts_before_specific_deny() {
        let rules_path =
            std::env::temp_dir().join("trusttunnel_append_allow_rule_specific_deny.toml");
        std::fs::write(
            &rules_path,
            "[[rule]]\ncidr = \"192.168.0.0/16\"\naction = \"deny\"\n",
        )
        .unwrap();

        append_allow_rule(&rules_path, "abcd/fff0").unwrap();

        let contents = std::fs::read_to_string(&rules_path).unwrap();
        let allow_pos = contents.find("client_random_prefix").unwrap();
        let deny_pos = contents.find("action = \"deny\"").unwrap();
        assert!(
            allow_pos < deny_pos,
            "allow rule should appear before any existing rule"
        );

        let _ = std::fs::remove_file(&rules_path);
    }
}
