use crate::acme::{
    issue_certificate, validate_domain, validate_email, AcmeConfig, ChallengeMethod, IssuedCert,
};
use crate::user_interaction::{ask_for_agreement, ask_for_input, checked_overwrite, select_index};
use crate::Mode;
use chrono::{Datelike, Duration, Local};
use rcgen::{BasicConstraints, DnType, IsCa};
use std::fs;
use std::io::Write;
use std::path::Path;
use trusttunnel::settings::{TlsHostInfo, TlsHostsSettings};
use trusttunnel::utils;
use trusttunnel::utils::Either;
use x509_parser::extensions::GeneralName;

const DEFAULT_CERTIFICATE_DURATION_DAYS: u64 = 365;
const DEFAULT_CERTIFICATE_FOLDER: &str = "certs";
const DEFAULT_HOSTNAME: &str = "vpn.endpoint";

pub struct TlsHostsSettingsResult {
    pub settings: TlsHostsSettings,
    pub cert_path: String,
    pub key_path: String,
}

pub fn build() -> TlsHostsSettingsResult {
    loop {
        if let Some(cert) = build_with_runtime() {
            let cert_path = cert.cert_path.clone();
            let key_path = cert.key_path.clone();
            return TlsHostsSettingsResult {
                settings: build_settings_from_cert(cert),
                cert_path,
                key_path,
            };
        }
        // In non-interactive mode, we can't retry
        if crate::get_mode() == Mode::NonInteractive {
            panic!("Certificate is required in non-interactive mode");
        }
        println!("\nNo certificate was created. Let's try again.\n");
    }
}

pub fn build_with_runtime() -> Option<Cert> {
    // Check for non-interactive mode with ACME parameters
    if crate::get_mode() == Mode::NonInteractive {
        // Check if Let's Encrypt is requested via CLI
        if let Some(ref cert_type) = crate::get_predefined_params().cert_type {
            if cert_type == "letsencrypt" {
                return generate_letsencrypt_cert_noninteractive();
            }
        }
        // Default to self-signed for non-interactive
        return generate_cert();
    }

    // Interactive mode
    lookup_existent_cert()
        .and_then(|x| {
            ask_for_agreement(&format!("Use an existing certificate? {:?}", x)).then_some(x)
        })
        .or_else(|| {
            let options = [
                "Issue a Let's Encrypt certificate (requires a public domain)",
                "Generate a self-signed certificate",
                "Provide path to existing certificate",
            ];

            let selection = select_index(
                "How would you like to create a certificate?",
                &options,
                Some(0),
            );

            match selection {
                0 => generate_letsencrypt_cert(),
                1 => generate_cert(),
                2 => ask_for_existing_cert(),
                _ => unreachable!(),
            }
        })
}

fn ask_for_existing_cert() -> Option<Cert> {
    let pair = ask_for_input::<String>(
        "Path to certificate file(s):\n  \
         - Single file containing both cert and key: /path/to/combined.pem\n  \
         - Separate files: /path/to/cert.pem /path/to/key.pem\n",
        None,
    );

    let mut iter = pair.splitn(2, char::is_whitespace);
    let x = match (iter.next().unwrap(), iter.next()) {
        (a, None) => Either::Left(a),
        (a, Some(b)) => Either::Right((a, b)),
    };

    let x = parse_cert(x);
    if x.is_none() {
        println!("Couldn't parse the provided key/certificate pair");
    }
    x
}

fn build_settings_from_cert(cert: Cert) -> TlsHostsSettings {
    let hostname = cert.common_name.clone();
    let allowed_sni = ask_for_alternative_snis();

    TlsHostsSettings::builder()
        .main_hosts(vec![TlsHostInfo {
            hostname: hostname.clone(),
            cert_chain_path: cert.cert_path.clone(),
            private_key_path: cert.key_path.clone(),
            allowed_sni,
        }])
        .build()
        .expect("Couldn't build TLS hosts settings")
}

#[derive(Debug, Clone)]
pub struct Cert {
    common_name: String,
    #[allow(dead_code)] // needed only for logging
    alt_names: Vec<String>,
    #[allow(dead_code)] // needed only for logging
    expiration_date: String,
    cert_path: String,
    key_path: String,
}

fn lookup_existent_cert() -> Option<Cert> {
    let files = fs::read_dir(DEFAULT_CERTIFICATE_FOLDER)
        .ok()?
        .filter_map(Result::ok)
        .filter(|entry| {
            entry
                .metadata()
                .map(|meta| meta.is_file())
                .unwrap_or_default()
        })
        .filter_map(|entry| entry.path().to_str().map(String::from))
        .collect::<Vec<_>>();

    let cert_key_pair = match files.as_slice() {
        [a] => Either::Left(a.as_str()),
        [a, b] => Either::Right((a.as_str(), b.as_str())),
        _ => return None,
    };

    parse_cert(cert_key_pair)
}

fn print_cert_error(path: &str, error: &std::io::Error) {
    let message = match error.kind() {
        std::io::ErrorKind::PermissionDenied => {
            format!("Permission denied: cannot read '{}'", path)
        }
        std::io::ErrorKind::NotFound => {
            format!("File not found: '{}'", path)
        }
        std::io::ErrorKind::InvalidInput => {
            format!("Invalid certificate or key format in '{}': {}", path, error)
        }
        _ => {
            format!("Failed to read '{}': {}", path, error)
        }
    };
    eprintln!("Error: {}", message);
}

fn parse_cert(cert: Either<&str, (&str, &str)>) -> Option<Cert> {
    let (chain, cert_path, key_path) = cert.map(
        |pair| {
            Some((
                utils::load_private_key(pair)
                    .and_then(|_| utils::load_certs(pair))
                    .map_err(|e| print_cert_error(pair, &e))
                    .ok()?,
                pair,
                pair,
            ))
        },
        |(a, b)| match (
            utils::load_certs(a),
            utils::load_private_key(b),
            utils::load_certs(b),
            utils::load_private_key(a),
        ) {
            (Ok(chain), Ok(_), _, _) => Some((chain, a, b)),
            (_, _, Ok(chain), Ok(_)) => Some((chain, b, a)),
            (Err(e), _, _, _) => {
                print_cert_error(a, &e);
                None
            }
            (_, Err(e), _, _) => {
                print_cert_error(b, &e);
                None
            }
        },
    )?;

    let cert = x509_parser::parse_x509_certificate(chain.first()?.0.as_slice())
        .ok()?
        .1;
    Some(Cert {
        common_name: cert.validity.is_valid().then(|| {
            let x = cert.subject.to_string();
            x.as_str()
                .strip_prefix("CN=")
                .map(String::from)
                .unwrap_or(x)
        })?,
        alt_names: cert
            .subject_alternative_name()
            .ok()
            .flatten()
            .map(|x| {
                x.value
                    .general_names
                    .iter()
                    .map(GeneralName::to_string)
                    .collect()
            })
            .unwrap_or_default(),
        expiration_date: cert.validity.not_after.to_string(),
        cert_path: cert_path.into(),
        key_path: key_path.into(),
    })
}

fn generate_cert() -> Option<Cert> {
    let (common_name, alt_names) = {
        println!("Let's generate a self-signed certificate.");
        let name = crate::get_predefined_params()
            .hostname
            .clone()
            .unwrap_or_else(|| {
                ask_for_input::<String>(
                    "Endpoint hostname (used for serving TLS connections)",
                    Some(DEFAULT_HOSTNAME.into()),
                )
            });
        (name.clone(), vec![name.clone(), format!("*.{}", name)])
    };
    let key_pair = rcgen::KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256)
        .expect("Failed to generate key pair");

    let mut params = rcgen::CertificateParams::new(alt_names.clone()).unwrap();
    let now = chrono::Local::now();
    let end_date = now
        .checked_add_days(chrono::Days::new(DEFAULT_CERTIFICATE_DURATION_DAYS))
        .unwrap();
    params.not_before = rcgen::date_time_ymd(now.year(), now.month() as u8, now.day() as u8);
    params.not_after = rcgen::date_time_ymd(
        end_date.year(),
        end_date.month() as u8,
        end_date.day() as u8,
    );
    params
        .distinguished_name
        .push(DnType::CommonName, &common_name);
    // CRITICAL: Set is_ca to true so Android accepts it as a CA certificate
    params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);

    let cert = params
        .self_signed(&key_pair)
        .expect("Failed to generate self-signed cert");
    let cert_path = format!("{DEFAULT_CERTIFICATE_FOLDER}/cert.pem");
    if !checked_overwrite(&cert_path, "Overwrite the existing certificate file?") {
        return None;
    }

    let key_path = format!("{DEFAULT_CERTIFICATE_FOLDER}/key.pem");
    if !checked_overwrite(&key_path, "Overwrite the existing private key file?") {
        return None;
    }

    fs::create_dir_all(Path::new(&cert_path).parent().unwrap())
        .expect("Couldn't create certificate directory path");
    fs::write(&cert_path, cert.pem()).expect("Couldn't write the certificate into a file");
    println!("The generated certificate is stored in file: {}", cert_path);

    fs::create_dir_all(Path::new(&key_path).parent().unwrap())
        .expect("Couldn't create private key directory path");
    if key_path != cert_path {
        fs::write(key_path.clone(), key_pair.serialize_pem())
            .expect("Couldn't write the private key into a file");
    } else {
        fs::OpenOptions::new()
            .append(true)
            .open(key_path.clone())
            .expect("Couldn't open a file for writing the private key")
            .write_all(key_pair.serialize_pem().as_bytes())
            .expect("Couldn't write the private key into a file");
    }
    println!("The generated private key is stored in file: {}", key_path);

    Some(Cert {
        common_name,
        alt_names,
        expiration_date: end_date.to_string(),
        cert_path,
        key_path,
    })
}

fn save_issued_cert(issued: IssuedCert, interactive: bool) -> Option<Cert> {
    let cert_path = format!("{}/cert.pem", DEFAULT_CERTIFICATE_FOLDER);
    let key_path = format!("{}/key.pem", DEFAULT_CERTIFICATE_FOLDER);

    if interactive {
        if !checked_overwrite(&cert_path, "Overwrite the existing certificate file?") {
            return None;
        }
        if !checked_overwrite(&key_path, "Overwrite the existing private key file?") {
            return None;
        }
    }

    fs::create_dir_all(DEFAULT_CERTIFICATE_FOLDER).expect("Couldn't create certificate directory");

    fs::write(&cert_path, &issued.cert_pem).expect("Couldn't write the certificate to file");
    println!("Certificate saved to: {}", cert_path);

    fs::write(&key_path, &issued.key_pem).expect("Couldn't write the private key to file");
    println!("Private key saved to: {}", key_path);

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(metadata) = fs::metadata(&key_path) {
            let mut perms = metadata.permissions();
            perms.set_mode(0o600);
            fs::set_permissions(&key_path, perms).ok();
        }
    }

    let expiration_date = parse_cert_expiration(&issued.cert_pem).unwrap_or_else(|| {
        Local::now()
            .checked_add_signed(Duration::days(90))
            .map(|d| d.format("%Y-%m-%d").to_string())
            .unwrap_or_else(|| "90 days from now".to_string())
    });

    Some(Cert {
        common_name: issued.domain.clone(),
        alt_names: vec![issued.domain],
        expiration_date,
        cert_path,
        key_path,
    })
}

fn generate_letsencrypt_cert() -> Option<Cert> {
    println!("Let's issue a Let's Encrypt certificate.");

    // Get domain name
    let domain: String = loop {
        let domain: String =
            ask_for_input("Enter your domain name (must be publicly accessible)", None);
        if validate_domain(&domain) {
            break domain;
        }
        println!(
            "Invalid domain format. Please enter a valid domain name (e.g., vpn.example.com)."
        );
    };

    // Get email address
    let email: String = loop {
        let email: String = ask_for_input(
            "Enter your email address (for Let's Encrypt notifications)",
            None,
        );
        if validate_email(&email) {
            break email;
        }
        println!("Invalid email format. Please try again.");
    };

    // Select challenge method
    let challenge_options = [
        "HTTP-01 (requires port 80 accessible from internet)",
        "DNS-01 (requires adding a TXT record to your DNS)",
    ];
    let challenge_selection = select_index("Select challenge method", &challenge_options, Some(0));
    let challenge_method = match challenge_selection {
        0 => ChallengeMethod::Http01,
        1 => ChallengeMethod::Dns01,
        _ => unreachable!(),
    };

    // Ask about staging environment
    let use_staging = ask_for_agreement(
        "Use Let's Encrypt staging environment for testing? (recommended for first attempt)",
    );

    if use_staging {
        println!("\n⚠️  Using staging environment. Certificate will NOT be trusted by browsers.");
        println!("   Run again without staging for a production certificate.\n");
    }

    let config = AcmeConfig {
        domain,
        email,
        challenge_method,
        use_staging,
    };

    // Run the async ACME flow
    let runtime = tokio::runtime::Runtime::new().expect("Failed to create tokio runtime");
    let result = runtime.block_on(issue_certificate(config));

    match result {
        Ok(issued) => save_issued_cert(issued, true),
        Err(ref e) => {
            println!("\n❌ Failed to issue Let's Encrypt certificate: {}", e);
            println!("\nPossible solutions:");
            match e {
                crate::acme::AcmeError::PortInUse(_) => {
                    println!("  • Stop any service using port 80, or");
                    println!("  • Use DNS-01 challenge instead");
                }
                crate::acme::AcmeError::ChallengeFailed(_) => {
                    println!("  • Verify your domain resolves to this server's IP");
                    println!("  • Check firewall allows inbound HTTP (port 80)");
                    println!("  • For DNS-01, ensure TXT record is correct and propagated");
                }
                _ => {
                    println!("  • Check your internet connection");
                    println!("  • Try using the staging environment first");
                }
            }

            if ask_for_agreement("Would you like to generate a self-signed certificate instead?") {
                generate_cert()
            } else {
                None
            }
        }
    }
}

fn generate_letsencrypt_cert_noninteractive() -> Option<Cert> {
    let predefined = crate::get_predefined_params();

    let domain = predefined
        .hostname
        .clone()
        .expect("Hostname is required for Let's Encrypt in non-interactive mode");
    if !validate_domain(&domain) {
        eprintln!("Invalid domain format: {}", domain);
        return None;
    }
    let email = predefined
        .acme_email
        .clone()
        .expect("ACME email is required for Let's Encrypt in non-interactive mode");
    if !validate_email(&email) {
        eprintln!("Invalid email format: {}", email);
        return None;
    }
    let challenge_method = predefined
        .acme_challenge
        .clone()
        .map(|s| {
            let method = s.parse::<ChallengeMethod>()
                .expect("Invalid challenge method");
            if method == ChallengeMethod::Dns01 {
                panic!("DNS-01 challenge is not supported in non-interactive mode (requires manual DNS record confirmation)");
            }
            method
        })
        .unwrap_or(ChallengeMethod::Http01);
    let use_staging = predefined.acme_staging;

    drop(predefined);

    if use_staging {
        println!("⚠️  Using Let's Encrypt staging environment");
    }

    let config = AcmeConfig {
        domain,
        email,
        challenge_method,
        use_staging,
    };

    let runtime = tokio::runtime::Runtime::new().expect("Failed to create tokio runtime");
    let result = runtime.block_on(issue_certificate(config));

    match result {
        Ok(issued) => save_issued_cert(issued, false),
        Err(e) => {
            eprintln!("Failed to issue Let's Encrypt certificate: {}", e);
            None
        }
    }
}

fn parse_cert_expiration(cert_pem: &str) -> Option<String> {
    let (_, pem) = x509_parser::pem::parse_x509_pem(cert_pem.as_bytes()).ok()?;
    let (_, cert) = x509_parser::parse_x509_certificate(&pem.contents).ok()?;
    let not_after = cert.validity.not_after.to_datetime();
    Some(format!(
        "{:04}-{:02}-{:02}",
        not_after.year(),
        not_after.month(),
        not_after.day()
    ))
}

fn ask_for_alternative_snis() -> Vec<String> {
    if crate::get_mode() == Mode::NonInteractive {
        return vec![];
    }

    if !ask_for_agreement("Do you want to configure alternative SNIs?") {
        return vec![];
    }

    let input: String = ask_for_input(
        "Enter alternative SNIs (comma-separated)",
        Some(String::new()),
    );

    if input.trim().is_empty() {
        return vec![];
    }

    input
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}
