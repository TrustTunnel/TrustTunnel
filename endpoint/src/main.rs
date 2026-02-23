use log::{debug, error, info, warn, LevelFilter};
use std::net::SocketAddr;
use std::str::FromStr;
use std::sync::Arc;
use tokio::signal;
use trusttunnel::authentication::jwt::JwtAuth;
use trusttunnel::authentication::mixed::MixedAuth;
use trusttunnel::authentication::registry_based::CredentialsAuth;
use trusttunnel::authentication::{AuthProvider, Authenticator, ProxyBasicAuthenticator};
use trusttunnel::client_config;
use trusttunnel::core::Core;
use trusttunnel::settings::{AuthMode, Settings};
use trusttunnel::shutdown::Shutdown;
use trusttunnel::{log_utils, settings};

const VERSION_STRING: &str = env!("CARGO_PKG_VERSION");
const VERSION_PARAM_NAME: &str = "v_e_r_s_i_o_n_do_not_change_this_name_it_will_break";
const LOG_LEVEL_PARAM_NAME: &str = "log_level";
const LOG_FILE_PARAM_NAME: &str = "log_file";
const SETTINGS_PARAM_NAME: &str = "settings";
const TLS_HOSTS_SETTINGS_PARAM_NAME: &str = "tls_hosts_settings";
const CLIENT_CONFIG_PARAM_NAME: &str = "client_config";
const ADDRESS_PARAM_NAME: &str = "address";
const CUSTOM_SNI_PARAM_NAME: &str = "custom_sni";
const CLIENT_RANDOM_PREFIX_PARAM_NAME: &str = "client_random_prefix";
const FORMAT_PARAM_NAME: &str = "format";
const SENTRY_DSN_PARAM_NAME: &str = "sentry_dsn";
const THREADS_NUM_PARAM_NAME: &str = "threads_num";

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
                .help("Endpoint address to be added to client's config."),
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
            clap::Arg::new(FORMAT_PARAM_NAME)
                .action(clap::ArgAction::Set)
                .requires(CLIENT_CONFIG_PARAM_NAME)
                .short('f')
                .long("format")
                .value_parser(["toml", "deeplink"])
                .default_value("deeplink")
                .help("Output format for client configuration: 'deeplink' produces tt:// URI, 'toml' produces traditional config file")
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
    let settings: Settings = toml::from_str(
        &std::fs::read_to_string(settings_path).expect("Couldn't read the settings file"),
    )
    .expect("Couldn't parse the settings file");

    if settings.get_clients().is_empty()
        && matches!(
            settings.get_auth().get_mode(),
            &AuthMode::Credentials | &AuthMode::Mixed
        )
        && settings.get_listen_address().ip().is_loopback()
    {
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
        let addresses: Vec<SocketAddr> = args
            .get_many::<String>(ADDRESS_PARAM_NAME)
            .expect("At least one address should be specified")
            .map(|x| {
                SocketAddr::from_str(x)
                    .or_else(|_| {
                        SocketAddr::from_str(&format!("{}:{}", x, settings.get_listen_address().port()))
                    })
                    .unwrap_or_else(|_| {
                        panic!("Failed to parse address. Expected `ip` or `ip:port` format, found: `{}`", x);
                    })
            })
            .collect();

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

        let mut client_random_prefix = args
            .get_one::<String>(CLIENT_RANDOM_PREFIX_PARAM_NAME)
            .cloned();
        if let Some(ref prefix) = client_random_prefix {
            // Validate hex format
            if hex::decode(prefix).is_err() {
                eprintln!("Error: client_random_prefix '{}' is not valid hex", prefix);
                std::process::exit(1);
            }

            // Validate against rules.toml
            if let Some(rules_engine) = settings.get_rules_engine() {
                let has_matching_rule = rules_engine.config().rule.iter().any(|rule| {
                    rule.client_random_prefix
                        .as_ref()
                        .map(|p| {
                            // Handle both "prefix" and "prefix/mask" formats
                            if let Some(slash) = p.find('/') {
                                &p[..slash] == prefix
                            } else {
                                p == prefix
                            }
                        })
                        .unwrap_or(false)
                });

                // Print warning and continue, do not panic because it's optional field
                if !has_matching_rule {
                    eprintln!(
                        "Warning: No rule found in rules.toml matching client_random_prefix '{}'. This field will be ignored.",
                        prefix
                    );
                    client_random_prefix = None;
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
                Ok(deep_link) => println!("{}", deep_link),
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
    let authenticator: Option<Arc<dyn Authenticator>> = {
        let auth_provider: Option<Box<dyn AuthProvider>> = match *settings.get_auth().get_mode() {
            AuthMode::Credentials => {
                if settings.get_clients().is_empty() {
                    None
                } else {
                    Some(Box::new(CredentialsAuth::new(settings.get_clients())))
                }
            }
            AuthMode::Jwt => {
                let jwt_settings = settings
                    .get_auth()
                    .get_jwt()
                    .as_ref()
                    .expect("[auth.jwt] must be configured for mode=jwt");
                let mut jwt_auth_config = jwt_settings.to_auth_config();
                jwt_auth_config.metrics_jwt_error_enabled = settings
                    .get_metrics()
                    .as_ref()
                    .map_or(true, |x| x.get_jwt_error_enabled().to_owned());
                Some(Box::new(
                    JwtAuth::from_config(&jwt_auth_config).expect("Couldn't initialize JWT auth"),
                ))
            }
            AuthMode::Mixed => {
                let jwt_settings = settings
                    .get_auth()
                    .get_jwt()
                    .as_ref()
                    .expect("[auth.jwt] must be configured for mode=mixed");
                let mut jwt_auth_config = jwt_settings.to_auth_config();
                jwt_auth_config.metrics_jwt_error_enabled = settings
                    .get_metrics()
                    .as_ref()
                    .map_or(true, |x| x.get_jwt_error_enabled().to_owned());
                Some(Box::new(MixedAuth::new(
                    Box::new(
                        JwtAuth::from_config(&jwt_auth_config)
                            .expect("Couldn't initialize JWT auth"),
                    ),
                    Box::new(CredentialsAuth::new(settings.get_clients())),
                )))
            }
        };

        auth_provider.map(|provider| {
            Arc::new(ProxyBasicAuthenticator::new(provider)) as Arc<dyn Authenticator>
        })
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
