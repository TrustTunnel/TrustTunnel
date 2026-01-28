use crate::user_interaction::{ask_for_agreement, ask_for_input, checked_overwrite};
use std::fs;
use std::io::IsTerminal;
use std::sync::{Mutex, MutexGuard};
use trusttunnel::settings::{Settings, TlsHostsSettings};

mod acme;
mod acme_http_server;
mod composer;
mod library_settings;
mod rules_settings;
mod template_settings;
mod tls_hosts_settings;
mod user_interaction;

const MODE_PARAM_NAME: &str = "mode";
const MODE_NON_INTERACTIVE: &str = "non-interactive";
const LISTEN_ADDRESS_PARAM_NAME: &str = "addr";
const CREDENTIALS_PARAM_NAME: &str = "creds";
const HOSTNAME_PARAM_NAME: &str = "host";
const LIBRARY_SETTINGS_FILE_PARAM_NAME: &str = "lib_settings";
const TLS_HOSTS_SETTINGS_FILE_PARAM_NAME: &str = "hosts_settings";
const CERT_TYPE_PARAM_NAME: &str = "cert_type";
const ACME_EMAIL_PARAM_NAME: &str = "acme_email";
const ACME_CHALLENGE_PARAM_NAME: &str = "acme_challenge";
const ACME_STAGING_PARAM_NAME: &str = "acme_staging";

#[derive(Clone, Copy, Debug, Ord, PartialOrd, Eq, PartialEq)]
pub enum Mode {
    NonInteractive,
    Interactive,
}

static MODE: Mutex<Mode> = Mutex::new(Mode::Interactive);

pub fn get_mode() -> Mode {
    *MODE.lock().unwrap()
}

#[derive(Default)]
pub struct PredefinedParameters {
    pub listen_address: Option<String>,
    pub credentials: Option<(String, String)>,
    pub hostname: Option<String>,
    pub library_settings_file: Option<String>,
    pub tls_hosts_settings_file: Option<String>,
    pub cert_type: Option<String>,
    pub acme_email: Option<String>,
    pub acme_challenge: Option<String>,
    pub acme_staging: bool,
}

lazy_static::lazy_static! {
    pub static ref PREDEFINED_PARAMS: Mutex<PredefinedParameters> = Mutex::default();
}

pub fn get_predefined_params() -> MutexGuard<'static, PredefinedParameters> {
    PREDEFINED_PARAMS.lock().unwrap()
}

fn main() {
    let args = clap::Command::new("VPN endpoint setup wizard")
        .about("Generate configuration files for TrustTunnel endpoint")
        .after_help(
            r#"EXAMPLES:
    # Interactive setup (recommended for first-time users)
    sudo ./setup_wizard

    # Non-interactive setup (for scripting/CI)
    sudo ./setup_wizard -m non-interactive \
        -a 0.0.0.0:443 \
        -c admin:secretpass \
        -n vpn.example.com \
        --lib-settings vpn.toml \
        --hosts-settings hosts.toml

    # After setup, export client configuration:
    sudo ./trusttunnel_endpoint vpn.toml hosts.toml -c admin -a 203.0.113.1

For detailed configuration options, see:
https://github.com/TrustTunnel/TrustTunnel/blob/master/CONFIGURATION.md
"#,
        )
        .disable_colored_help(false)
        .args(&[
            clap::Arg::new(MODE_PARAM_NAME)
                .short('m')
                .long("mode")
                .action(clap::ArgAction::Set)
                .value_parser(["interactive", MODE_NON_INTERACTIVE])
                .default_value("interactive")
                .help(
                    r#"Available wizard running modes:
    * interactive - set up only the essential without deep diving into details
    * non-interactive - prepare the setup without interacting with a user,
                        requires some parameters set up via command-line arguments
"#,
                ),
            clap::Arg::new(LISTEN_ADDRESS_PARAM_NAME)
                .short('a')
                .long("address")
                .action(clap::ArgAction::Set)
                .value_parser(clap::builder::NonEmptyStringValueParser::new())
                .required_if_eq(MODE_PARAM_NAME, MODE_NON_INTERACTIVE)
                .help(Settings::doc_listen_address()),
            clap::Arg::new(CREDENTIALS_PARAM_NAME)
                .short('c')
                .long("creds")
                .action(clap::ArgAction::Set)
                .value_parser(clap::builder::NonEmptyStringValueParser::new())
                .required_if_eq(MODE_PARAM_NAME, MODE_NON_INTERACTIVE)
                .help(
                    r#"A user credentials formatted as: <username>:<password>.
Required in non-interactive mode."#,
                ),
            clap::Arg::new(HOSTNAME_PARAM_NAME)
                .short('n')
                .long("hostname")
                .action(clap::ArgAction::Set)
                .value_parser(clap::builder::NonEmptyStringValueParser::new())
                .required_if_eq(MODE_PARAM_NAME, MODE_NON_INTERACTIVE)
                .help(
                    r#"A hostname of the certificate for serving TLS connections.
Required in non-interactive mode."#,
                ),
            clap::Arg::new(LIBRARY_SETTINGS_FILE_PARAM_NAME)
                .long("lib-settings")
                .action(clap::ArgAction::Set)
                .value_parser(clap::builder::NonEmptyStringValueParser::new())
                .required_if_eq(MODE_PARAM_NAME, MODE_NON_INTERACTIVE)
                .help("Path to store the library settings file. Required in non-interactive mode."),
            clap::Arg::new(TLS_HOSTS_SETTINGS_FILE_PARAM_NAME)
                .long("hosts-settings")
                .action(clap::ArgAction::Set)
                .value_parser(clap::builder::NonEmptyStringValueParser::new())
                .required_if_eq(MODE_PARAM_NAME, MODE_NON_INTERACTIVE)
                .help(
                    "Path to store the TLS hosts settings file. Required in non-interactive mode.",
                ),
            clap::Arg::new(CERT_TYPE_PARAM_NAME)
                .long("cert-type")
                .action(clap::ArgAction::Set)
                .value_parser(["self-signed", "letsencrypt"])
                .help("Certificate type: 'self-signed' or 'letsencrypt'"),
            clap::Arg::new(ACME_EMAIL_PARAM_NAME)
                .long("acme-email")
                .action(clap::ArgAction::Set)
                .value_parser(clap::builder::NonEmptyStringValueParser::new())
                .required_if_eq(CERT_TYPE_PARAM_NAME, "letsencrypt")
                .help("Email address for Let's Encrypt account (required when --cert-type=letsencrypt)"),
            clap::Arg::new(ACME_CHALLENGE_PARAM_NAME)
                .long("acme-challenge")
                .action(clap::ArgAction::Set)
                .value_parser(["http-01"])
                .default_value("http-01")
                .help("ACME challenge method: 'http-01' (dns-01 is only available in interactive mode)"),
            clap::Arg::new(ACME_STAGING_PARAM_NAME)
                .long("acme-staging")
                .action(clap::ArgAction::SetTrue)
                .help("Use Let's Encrypt staging environment (for testing)"),
        ])
        .get_matches();

    *MODE.lock().unwrap() = match args.get_one::<String>(MODE_PARAM_NAME).map(String::as_str) {
        None => Mode::Interactive,
        Some(MODE_NON_INTERACTIVE) => Mode::NonInteractive,
        Some("interactive") => Mode::Interactive,
        _ => unreachable!(),
    };

    if *MODE.lock().unwrap() == Mode::Interactive && !std::io::stdin().is_terminal() {
        eprintln!("Error: Interactive mode requires a terminal (TTY).");
        eprintln!("Please run setup_wizard from a terminal, or use non-interactive mode:");
        eprintln!("  {} --help", std::env::args().next().unwrap_or_default());
        std::process::exit(1);
    }

    *PREDEFINED_PARAMS.lock().unwrap() = PredefinedParameters {
        listen_address: args.get_one::<String>(LISTEN_ADDRESS_PARAM_NAME).cloned(),
        credentials: args
            .get_one::<String>(CREDENTIALS_PARAM_NAME)
            .map(|x| x.splitn(2, ':'))
            .and_then(|mut x| x.next().zip(x.next()))
            .map(|(a, b)| (a.to_string(), b.to_string()))
            .map(|(username, password)| {
                if username.is_empty() {
                    eprintln!("Error: Username cannot be empty");
                    std::process::exit(1);
                }
                if password.is_empty() {
                    eprintln!("Error: Password cannot be empty");
                    std::process::exit(1);
                }
                (username, password)
            }),
        hostname: args.get_one::<String>(HOSTNAME_PARAM_NAME).cloned(),
        library_settings_file: args
            .get_one::<String>(LIBRARY_SETTINGS_FILE_PARAM_NAME)
            .cloned(),
        tls_hosts_settings_file: args
            .get_one::<String>(TLS_HOSTS_SETTINGS_FILE_PARAM_NAME)
            .cloned(),
        cert_type: args.get_one::<String>(CERT_TYPE_PARAM_NAME).cloned(),
        acme_email: args.get_one::<String>(ACME_EMAIL_PARAM_NAME).cloned(),
        acme_challenge: args.get_one::<String>(ACME_CHALLENGE_PARAM_NAME).cloned(),
        acme_staging: args.get_flag(ACME_STAGING_PARAM_NAME),
    };

    println!("Welcome to the setup wizard");

    let library_settings_path = find_existent_settings::<Settings>(".")
        .and_then(|fname| {
            ask_for_agreement(&format!("Use the existing library settings {}?", fname))
                .then_some(fname)
        })
        .or_else(|| {
            println!("Let's build the library settings");
            let built = library_settings::build();
            println!("The library settings are successfully built\n");

            let path = get_predefined_params()
                .library_settings_file
                .clone()
                .unwrap_or_else(|| {
                    ask_for_input::<String>(
                        "Path to a file to store the library settings",
                        Some("vpn.toml".into()),
                    )
                });
            if checked_overwrite(&path, "Overwrite the existing library settings file?") {
                let doc = composer::compose_document(
                    &built.settings,
                    &built.credentials_path,
                    &built.rules_path,
                );
                fs::write(&path, doc).expect("Couldn't write the library settings to a file");
            }
            Some(path)
        });

    let (hosts_settings_path, cert_path, key_path) =
        find_existent_settings::<TlsHostsSettings>(".")
            .and_then(|fname| {
                ask_for_agreement(&format!("Use the existing TLS hosts settings {}?", fname))
                    .then_some((fname, None, None))
            })
            .or_else(|| {
                println!("Let's build the TLS hosts settings");
                let result = tls_hosts_settings::build();
                println!("The TLS hosts settings are successfully built\n");

                let path = get_predefined_params()
                    .tls_hosts_settings_file
                    .clone()
                    .unwrap_or_else(|| {
                        ask_for_input::<String>(
                            "Path to a file to store the TLS hosts settings",
                            Some("hosts.toml".into()),
                        )
                    });
                if checked_overwrite(&path, "Overwrite the existing TLS hosts settings file?") {
                    fs::write(
                        &path,
                        toml::ser::to_string(&result.settings)
                            .expect("Couldn't serialize the TLS hosts settings"),
                    )
                    .expect("Couldn't write the TLS hosts settings to a file");
                }
                Some((path, Some(result.cert_path), Some(result.key_path)))
            })
            .map(|(p, c, k)| (Some(p), c, k))
            .unwrap_or((None, None, None));

    if let (Some(l), Some(h)) = (library_settings_path, hosts_settings_path) {
        print_setup_complete_summary(&l, &h, cert_path.as_deref(), key_path.as_deref());
    } else {
        println!("To see the full set of available options, run the following command:");
        println!("\ttrusttunnel_endpoint -h");
    }
}

fn print_setup_complete_summary(
    lib_settings_path: &str,
    hosts_settings_path: &str,
    cert_path: Option<&str>,
    key_path: Option<&str>,
) {
    println!();
    println!("═══════════════════════════════════════════════════════════════");
    println!("                    Setup Complete!");
    println!("═══════════════════════════════════════════════════════════════");
    println!();
    println!("Configuration files created:");
    println!(
        "  • {}          - Main endpoint settings",
        lib_settings_path
    );
    println!("  • {}    - TLS host configuration", hosts_settings_path);
    println!(
        "  • {}  - User credentials",
        library_settings::DEFAULT_CREDENTIALS_PATH
    );
    if let Some(cert) = cert_path {
        println!("  • {}    - TLS certificate", cert);
    }
    if let Some(key) = key_path {
        println!("  • {}     - TLS private key", key);
    }
    println!();
    println!("───────────────────────────────────────────────────────────────");
    println!("                      Next Steps");
    println!("───────────────────────────────────────────────────────────────");
    println!();
    println!("1. Start the endpoint:");
    println!(
        "   ./trusttunnel_endpoint {} {}",
        lib_settings_path, hosts_settings_path
    );
    println!();
    println!("2. Export client configuration (replace <username> and <public_ip>):");
    println!(
        "   ./trusttunnel_endpoint {} {} -c <username> -a <public_ip>:443",
        lib_settings_path, hosts_settings_path
    );
    println!();
    println!("3. Use the exported config with:");
    println!("   • TrustTunnel CLI Client - Pass to setup_wizard --endpoint_config");
    println!("   • TrustTunnel Flutter Client - Enter the config manually");
    println!();
    const CONFIG_URL: &str =
        "https://github.com/TrustTunnel/TrustTunnel/blob/master/CONFIGURATION.md";
    println!(
        "See \x1b]8;;{}\x1b\\{}\x1b]8;;\x1b\\ for advanced settings.",
        CONFIG_URL, CONFIG_URL
    );
    println!("═══════════════════════════════════════════════════════════════");
}

fn find_existent_settings<T: serde::de::DeserializeOwned>(path: &str) -> Option<String> {
    (get_mode() != Mode::NonInteractive)
        .then(|| {
            fs::read_dir(path)
                .ok()?
                .filter_map(Result::ok)
                .filter(|entry| {
                    entry
                        .metadata()
                        .map(|meta| meta.is_file())
                        .unwrap_or_default()
                })
                .filter_map(|entry| entry.file_name().into_string().ok())
                .filter_map(|fname| fs::read_to_string(&fname).ok().zip(Some(fname)))
                .find_map(|(content, fname)| toml::from_str::<T>(&content).map(|_| fname).ok())
        })
        .flatten()
}
