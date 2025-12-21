use crate::user_interaction::{ask_for_agreement, ask_for_input, checked_overwrite};
use crate::Mode;
use chrono::Datelike;
use rcgen::DnType;
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

pub fn build() -> TlsHostsSettings {
    let cert = (crate::get_predefined_params().hostname.is_some()
        || crate::get_mode() == Mode::NonInteractive)
        .then(generate_cert)
        .flatten()
        .or_else(|| {
            lookup_existent_cert().and_then(|x| {
                (crate::get_mode() != Mode::NonInteractive
                    && ask_for_agreement(&format!("Use an existing certificate? {:?}", x)))
                .then_some(x)
            })
        })
        .or_else(|| {
            ask_for_agreement("Generate a self-signed certificate?")
                .then(generate_cert)
                .flatten()
        })
        .or_else(|| {
            let pair = ask_for_input::<String>(
                "Path to key/certificate pair. Divide by space if they are in separate files.\n",
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
        });

    let hostname = cert
        .as_ref()
        .map(|x| x.common_name.clone())
        .or_else(|| crate::get_predefined_params().hostname.clone())
        .unwrap_or_else(|| {
            ask_for_input::<String>(
                "Endpoint hostname (used for serving TLS connections)",
                Some(DEFAULT_HOSTNAME.into()),
            )
        });

    TlsHostsSettings::builder()
        .main_hosts(vec![TlsHostInfo {
            hostname: hostname.clone(),
            cert_chain_path: cert.as_ref().unwrap().cert_path.clone(),
            private_key_path: cert.as_ref().unwrap().key_path.clone(),
        }])
        .ping_hosts(vec![TlsHostInfo {
            hostname: format!("ping.{}", hostname),
            cert_chain_path: cert.as_ref().unwrap().cert_path.clone(),
            private_key_path: cert.as_ref().unwrap().key_path.clone(),
        }])
        .speedtest_hosts(vec![TlsHostInfo {
            hostname: format!("speed.{}", hostname),
            cert_chain_path: cert.as_ref().unwrap().cert_path.clone(),
            private_key_path: cert.as_ref().unwrap().key_path.clone(),
        }])
        .build()
        .expect("Couldn't build TLS hosts settings")
}

#[derive(Debug)]
struct Cert {
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

fn parse_cert(cert: Either<&str, (&str, &str)>) -> Option<Cert> {
    let (chain, cert_path, key_path) = cert.map(
        |pair| {
            Some((
                utils::load_private_key(pair)
                    .and_then(|_| utils::load_certs(pair))
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
            _ => None,
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
    let mut params = rcgen::CertificateParams::new(alt_names.clone());
    params.alg = &rcgen::PKCS_ECDSA_P256_SHA256;
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

    let cert = rcgen::Certificate::from_params(params).unwrap();
    let cert_path = format!("{DEFAULT_CERTIFICATE_FOLDER}/cert.pem");
    if !checked_overwrite(&cert_path, "Overwrite the existing certificate file?") {
        return None;
    }

    let key_path = format!("{DEFAULT_CERTIFICATE_FOLDER}/key.pem");
    if !checked_overwrite(&cert_path, "Overwrite the existing private key file?") {
        return None;
    }

    fs::create_dir_all(Path::new(&cert_path).parent().unwrap())
        .expect("Couldn't create certificate directory path");
    fs::write(&cert_path, cert.serialize_pem().unwrap())
        .expect("Couldn't write the certificate into a file");
    println!("The generated certificate is stored in file: {}", cert_path);

    fs::create_dir_all(Path::new(&cert_path).parent().unwrap())
        .expect("Couldn't create private key directory path");
    if key_path != cert_path {
        fs::write(key_path.clone(), cert.serialize_private_key_pem())
            .expect("Couldn't write the private key into a file");
    } else {
        fs::OpenOptions::new()
            .write(true)
            .append(true)
            .open(key_path.clone())
            .expect("Couldn't open a file for writing the private key")
            .write_all(cert.serialize_private_key_pem().as_bytes())
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
