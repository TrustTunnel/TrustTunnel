use crate::user_interaction::{ask_for_agreement, ask_for_input, checked_overwrite};
use std::fs;
use std::sync::{Mutex, MutexGuard};
use trusttunnel::settings::{Settings, TlsHostsSettings};

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
    listen_address: Option<String>,
    credentials: Option<(String, String)>,
    hostname: Option<String>,
    library_settings_file: Option<String>,
    tls_hosts_settings_file: Option<String>,
}

lazy_static::lazy_static! {
    pub static ref PREDEFINED_PARAMS: Mutex<PredefinedParameters> = Mutex::default();
}

pub fn get_predefined_params() -> MutexGuard<'static, PredefinedParameters> {
    PREDEFINED_PARAMS.lock().unwrap()
}

fn main() {
    let args = clap::Command::new("VPN endpoint setup wizard")
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
        ])
        .get_matches();

    *MODE.lock().unwrap() = match args.get_one::<String>(MODE_PARAM_NAME).map(String::as_str) {
        None => Mode::Interactive,
        Some(MODE_NON_INTERACTIVE) => Mode::NonInteractive,
        Some("interactive") => Mode::Interactive,
        _ => unreachable!(),
    };

    *PREDEFINED_PARAMS.lock().unwrap() = PredefinedParameters {
        listen_address: args.get_one::<String>(LISTEN_ADDRESS_PARAM_NAME).cloned(),
        credentials: args
            .get_one::<String>(CREDENTIALS_PARAM_NAME)
            .map(|x| x.splitn(2, ':'))
            .and_then(|mut x| x.next().zip(x.next()))
            .map(|(a, b)| (a.to_string(), b.to_string())),
        hostname: args.get_one::<String>(HOSTNAME_PARAM_NAME).cloned(),
        library_settings_file: args
            .get_one::<String>(LIBRARY_SETTINGS_FILE_PARAM_NAME)
            .cloned(),
        tls_hosts_settings_file: args
            .get_one::<String>(TLS_HOSTS_SETTINGS_FILE_PARAM_NAME)
            .cloned(),
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

    let hosts_settings_path = find_existent_settings::<TlsHostsSettings>(".")
        .and_then(|fname| {
            ask_for_agreement(&format!("Use the existing TLS hosts settings {}?", fname))
                .then_some(fname)
        })
        .or_else(|| {
            println!("Let's build the TLS hosts settings");
            let settings = tls_hosts_settings::build();
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
                    toml::ser::to_string(&settings)
                        .expect("Couldn't serialize the TLS hosts settings"),
                )
                .expect("Couldn't write the TLS hosts settings to a file");
            }
            Some(path)
        });

    if let (Some(l), Some(h)) = (library_settings_path, hosts_settings_path) {
        println!("To start endpoint, run the following command:");
        println!("\ttrusttunnel_endpoint {} {}", l, h);
    }
    println!("To see the full set of available options, run the following command:");
    println!("\ttrusttunnel_endpoint -h");
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
