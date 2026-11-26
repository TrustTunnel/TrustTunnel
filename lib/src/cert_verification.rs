use crate::utils;
use rustls::client::danger::ServerCertVerifier;
use rustls::client::WebPkiServerVerifier;
use rustls::RootCertStore;
use rustls_pki_types::ServerName;
use std::io;
use std::sync::Arc;

/// Checks if certificate chain would be verifiable by system CAs.
///
/// The server-side check only determines whether to omit the certificate from
/// the deep-link to reduce its size. Security validation happens client-side
/// during deep-link import.
pub struct CertificateVerifier {
    root_store: Arc<RootCertStore>,
}

/// Check whether the certificate chain in `cert_path` is verified against
/// `hostname` by the system trust store. Returns `false` when the trust store
/// cannot be loaded.
pub fn is_system_verifiable(cert_path: &str, hostname: &str) -> bool {
    CertificateVerifier::new()
        .map(|verifier| verifier.is_system_verifiable(cert_path, hostname))
        .unwrap_or(false)
}

impl CertificateVerifier {
    /// Create a new verifier with system trust anchors loaded.
    pub fn new() -> io::Result<Self> {
        let mut root_store = RootCertStore::empty();

        let native_certs = rustls_native_certs::load_native_certs()
            .map_err(|e| io::Error::other(format!("failed to load system CAs: {}", e)))?;
        for cert in native_certs {
            root_store
                .add(cert)
                .map_err(|e| io::Error::other(format!("failed to add CA cert: {}", e)))?;
        }

        Ok(Self {
            root_store: Arc::new(root_store),
        })
    }

    /// Check if a certificate chain is verifiable by system CAs.
    ///
    /// Returns `true` if the certificate chain can be verified, `false` otherwise.
    /// This is used to determine if the certificate can be omitted from deep-links.
    ///
    /// # Arguments
    /// * `cert_path` - Path to the certificate chain file (PEM format)
    /// * `hostname` - The hostname to verify against
    pub fn is_system_verifiable(&self, cert_path: &str, hostname: &str) -> bool {
        // Load certificates from file
        let certs = match utils::load_certs(cert_path) {
            Ok(certs) => certs,
            Err(e) => {
                debug!("Failed to load certificates from {}: {}", cert_path, e);
                return false;
            }
        };

        if certs.is_empty() {
            debug!("No certificates found in {}", cert_path);
            return false;
        }

        // Parse hostname as ServerName
        let server_name = match ServerName::try_from(hostname.to_string()) {
            Ok(name) => name,
            Err(e) => {
                debug!("Invalid hostname {}: {}", hostname, e);
                return false;
            }
        };

        let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
        let verifier =
            match WebPkiServerVerifier::builder_with_provider(self.root_store.clone(), provider)
                .build()
            {
                Ok(v) => v,
                Err(e) => {
                    debug!("Failed to build verifier: {}", e);
                    return false;
                }
            };
        let end_entity = &certs[0];
        let intermediates: Vec<_> = certs.iter().skip(1).cloned().collect();
        let now = rustls_pki_types::UnixTime::now();

        match verifier.verify_server_cert(end_entity, &intermediates, &server_name, &[], now) {
            Ok(_) => {
                debug!("Certificate chain for {} is system-verifiable", hostname);
                true
            }
            Err(e) => {
                debug!(
                    "Certificate chain for {} is not system-verifiable: {}",
                    hostname, e
                );
                false
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    #[test]
    fn test_verifier_creation() {
        let verifier = CertificateVerifier::new();
        assert!(verifier.is_ok(), "Should be able to load system CAs");
    }

    #[test]
    fn test_self_signed_cert_not_verifiable() {
        let verifier = CertificateVerifier::new().unwrap();

        let self_signed_pem = r#"-----BEGIN CERTIFICATE-----
MIICpDCCAYwCCQDU+pQ3ZUD30jANBgkqhkiG9w0BAQsFADAUMRIwEAYDVQQDDAls
b2NhbGhvc3QwHhcNMjQwMTAxMDAwMDAwWhcNMjUwMTAxMDAwMDAwWjAUMRIwEAYD
VQQDDAlsb2NhbGhvc3QwggEiMA0GCSqGSIb3DQEBAQUAA4IBDwAwggEKAoIBAQC7
VJTUt9Us8cKjMzEfYyjiWA4R4/M2bS1+fWIcPm15A8SE0MNvYhggo4ExRbEW9dUg
YpSMEo5c4rF6VhqzNb8s6G8E2Yfl1hP8xECvH8VGCE1aEhD9yEV4YgDJTVfD7aL+
hBNDhjKQqPJq7L2xCBQm8KqTFsXjPWvqLy3L0eLCCNTPqQGNmjZ9YPqC2RLxXEhz
pV+9K2qI3qJ6lV0tQwVKPPZEJ/9KPZQF1zEivQJqv1+5+DH2lxU5SG7tEXe6S7F/
VLRVBiEA1sYmZWqFQ9Jc5qLqbEz1RvGGfWqPdHVhU4KOzxXPFALLNRDR0KjWGVCG
ljD7r2K7qNjLpF9cOXJHAgMBAAEwDQYJKoZIhvcNAQELBQADggEBAGQSqg==
-----END CERTIFICATE-----"#;

        let mut temp_file = NamedTempFile::new().unwrap();
        temp_file.write_all(self_signed_pem.as_bytes()).unwrap();
        temp_file.flush().unwrap();

        let result = verifier.is_system_verifiable(temp_file.path().to_str().unwrap(), "localhost");
        assert!(
            !result,
            "Self-signed certificate should not be system-verifiable"
        );
    }

    #[test]
    fn test_invalid_cert_path() {
        let verifier = CertificateVerifier::new().unwrap();
        let result = verifier.is_system_verifiable("/nonexistent/path/cert.pem", "example.com");
        assert!(!result, "Invalid cert path should return false");
    }

    #[test]
    fn test_invalid_hostname() {
        let verifier = CertificateVerifier::new().unwrap();

        let valid_pem = r#"-----BEGIN CERTIFICATE-----
MIICpDCCAYwCCQDU+pQ3ZUD30jANBgkqhkiG9w0BAQsFADAUMRIwEAYDVQQDDAls
b2NhbGhvc3QwHhcNMjQwMTAxMDAwMDAwWhcNMjUwMTAxMDAwMDAwWjAUMRIwEAYD
VQQDDAlsb2NhbGhvc3QwggEiMA0GCSqGSIb3DQEBAQUAA4IBDwAwggEKAoIBAQC7
VJTUt9Us8cKjMzEfYyjiWA4R4/M2bS1+fWIcPm15A8SE0MNvYhggo4ExRbEW9dUg
YpSMEo5c4rF6VhqzNb8s6G8E2Yfl1hP8xECvH8VGCE1aEhD9yEV4YgDJTVfD7aL+
hBNDhjKQqPJq7L2xCBQm8KqTFsXjPWvqLy3L0eLCCNTPqQGNmjZ9YPqC2RLxXEhz
pV+9K2qI3qJ6lV0tQwVKPPZEJ/9KPZQF1zEivQJqv1+5+DH2lxU5SG7tEXe6S7F/
VLRVBiEA1sYmZWqFQ9Jc5qLqbEz1RvGGfWqPdHVhU4KOzxXPFALLNRDR0KjWGVCG
ljD7r2K7qNjLpF9cOXJHAgMBAAEwDQYJKoZIhvcNAQELBQADggEBAGQSqg==
-----END CERTIFICATE-----"#;

        let mut temp_file = NamedTempFile::new().unwrap();
        temp_file.write_all(valid_pem.as_bytes()).unwrap();
        temp_file.flush().unwrap();

        let result = verifier.is_system_verifiable(
            temp_file.path().to_str().unwrap(),
            "not a valid hostname!!!",
        );
        assert!(!result, "Invalid hostname should return false");
    }
}
