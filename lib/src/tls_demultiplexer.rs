use crate::net_utils::Channel;
use crate::settings::Settings;
use crate::{net_utils, settings, utils};
use boring::pkey::{PKey, Private};
use boring::rsa::Rsa;
use boring::x509::X509;
use rustls_pki_types::{CertificateDer, PrivateKeyDer};
use smallvec::SmallVec;
use std::collections::HashMap;
use std::fmt::{Debug, Formatter};
use std::io;
use std::sync::Arc;

const DEFAULT_PROTOCOL: Protocol = Protocol::Http1;

#[derive(Debug, Copy, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Protocol {
    Http1,
    Http2,
    Http3,
}

#[derive(Clone)]
pub(crate) struct BoringIdentity {
    pub chain: Arc<Vec<X509>>,
    pub key: Arc<PKey<Private>>,
}

struct Host {
    cert_chain: Vec<CertificateDer<'static>>,
    key: PrivateKeyDer<'static>,
    /// Quiche only accepts paths
    cert_chain_path: String,
    /// Quiche only accepts paths
    key_path: String,
    /// Alternative SNIs that should be accepted for this host
    allowed_sni: Vec<String>,
    /// Pre-parsed certificates and private key for boring SSL (performance optimization)
    boring: BoringIdentity,
}

pub(crate) struct ConnectionMeta {
    /// The server name a client sent in the client hello
    pub sni: String,
    /// The protocol selected by the demultiplexer
    pub protocol: Protocol,
    /// The channel selected by the demultiplexer
    pub channel: Channel,
    /// The certificate chain of the TLS server on the connection
    pub cert_chain: Vec<CertificateDer<'static>>,
    /// The private key of the TLS server on the connection
    pub key: PrivateKeyDer<'static>,
    /// Quiche only accepts paths
    pub cert_chain_path: String,
    /// Quiche only accepts paths
    pub key_path: String,
    /// The SNI-based authentication credentials is some
    pub sni_auth_creds: Option<String>,
    /// Pre-parsed certificates and private key for boring SSL (performance optimization)
    pub boring: BoringIdentity,
}

impl Clone for ConnectionMeta {
    fn clone(&self) -> Self {
        Self {
            sni: self.sni.clone(),
            protocol: self.protocol,
            channel: self.channel,
            cert_chain: self.cert_chain.clone(),
            key: self.key.clone_key(),
            cert_chain_path: self.cert_chain_path.clone(),
            key_path: self.key_path.clone(),
            sni_auth_creds: self.sni_auth_creds.clone(),
            boring: self.boring.clone(),
        }
    }
}

impl Debug for ConnectionMeta {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        let sni_temp: String;
        let sni_ref = if self.sni_auth_creds.is_some() {
            sni_temp = net_utils::scrub_sni(self.sni.clone());
            &sni_temp
        } else {
            &self.sni
        };
        write!(
            f,
            "ConnectionMeta {{ \
                   sni: \"{}\", \
                   protocol: {:?}, \
                   channel: {:?}, \
                   sni_auth_creds: {:?} \
               }}",
            sni_ref, self.protocol, self.channel, self.sni_auth_creds,
        )
    }
}

pub(crate) struct TlsDemux {
    main_hosts: HashMap<String, Host>,
    reverse_proxy_hosts: HashMap<String, Host>,
    ping_hosts: HashMap<String, Host>,
    speedtest_hosts: HashMap<String, Host>,
    subscription_hosts: HashMap<String, Host>,
    tunnel_protocols: SmallVec<[Protocol; 3]>,
    allowed_sni_to_main_host: HashMap<String, String>,
}

impl Protocol {
    pub fn as_alpn(&self) -> &'static str {
        match self {
            Self::Http1 => net_utils::HTTP1_ALPN,
            Self::Http2 => net_utils::HTTP2_ALPN,
            Self::Http3 => net_utils::HTTP3_ALPN,
        }
    }

    fn from_alpn(alpn: &str) -> Option<Self> {
        match alpn {
            net_utils::HTTP1_ALPN => Some(Protocol::Http1),
            net_utils::HTTP2_ALPN => Some(Protocol::Http2),
            net_utils::HTTP3_ALPN => Some(Protocol::Http3),
            _ => None,
        }
    }
}

impl Protocol {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Http1 => "HTTP1",
            Self::Http2 => "HTTP2",
            Self::Http3 => "HTTP3",
        }
    }
}

impl TlsDemux {
    pub fn new(settings: &Settings, tls_settings: &settings::TlsHostsSettings) -> io::Result<Self> {
        // false-positive
        #[allow(unused_variables)]
        let make_entry = |x: &settings::TlsHostInfo| -> io::Result<(String, Host)> {
            let cert_chain = if cfg!(test) {
                Default::default()
            } else {
                utils::load_certs(&x.cert_chain_path)?
            };

            let key = if cfg!(test) {
                PrivateKeyDer::Pkcs8(rustls_pki_types::PrivatePkcs8KeyDer::from(vec![]))
            } else {
                utils::load_private_key(&x.private_key_path)?
            };

            let boring = if cfg!(test) {
                // Create dummy BoringIdentity for tests
                let rsa = Rsa::generate(2048).unwrap();
                let pkey = PKey::from_rsa(rsa).unwrap();
                BoringIdentity {
                    chain: Arc::new(Vec::new()),
                    key: Arc::new(pkey),
                }
            } else {
                let mut chain = Vec::with_capacity(cert_chain.len());
                for c in &cert_chain {
                    chain.push(
                        X509::from_der(c.as_ref())
                            .map_err(|e| io::Error::other(format!("X509 parse error: {e}")))?,
                    );
                }

                let key_bytes = key.secret_der();
                let boring_key: PKey<Private> = PKey::private_key_from_der(key_bytes)
                    .or_else(|_| PKey::private_key_from_pkcs8(key_bytes))
                    .or_else(|_| PKey::private_key_from_pem(key_bytes))
                    .map_err(|e| io::Error::other(format!("PKey parse error: {e}")))?;

                BoringIdentity {
                    chain: Arc::new(chain),
                    key: Arc::new(boring_key),
                }
            };

            Ok((
                x.hostname.clone(),
                Host {
                    cert_chain,
                    key,
                    cert_chain_path: x.cert_chain_path.clone(),
                    key_path: x.private_key_path.clone(),
                    allowed_sni: x.allowed_sni.clone(),
                    boring,
                },
            ))
        };

        macro_rules! make_hosts {
            ($list: expr) => {
                $list.iter().map(make_entry).collect::<io::Result<_>>()
            };
        }

        let main_hosts: HashMap<String, Host> = make_hosts!(tls_settings.main_hosts)?;

        let allowed_sni_to_main_host: HashMap<String, String> = main_hosts
            .iter()
            .flat_map(|(hostname, host)| {
                host.allowed_sni
                    .iter()
                    .map(move |sni| (sni.clone(), hostname.clone()))
            })
            .collect();

        Ok(Self {
            main_hosts,
            ping_hosts: make_hosts!(tls_settings.ping_hosts)?,
            speedtest_hosts: make_hosts!(tls_settings.speedtest_hosts)?,
            reverse_proxy_hosts: match settings.reverse_proxy {
                None => Default::default(),
                Some(_) => make_hosts!(tls_settings.reverse_proxy_hosts)?,
            },
            subscription_hosts: match settings.subscription {
                None => Default::default(),
                Some(_) => make_hosts!(tls_settings.subscription_hosts)?,
            },
            tunnel_protocols: {
                let mut x = SmallVec::new();
                if settings.listen_protocols.http1.is_some() {
                    x.push(Protocol::Http1);
                }
                if settings.listen_protocols.http2.is_some() {
                    x.push(Protocol::Http2);
                }
                if settings.listen_protocols.quic.is_some() {
                    x.push(Protocol::Http3);
                }
                x
            },
            allowed_sni_to_main_host,
        })
    }

    /// There is no API method to get SNI from the client hello before accepting
    /// the connection. So try accepting it with the first certificate and change
    /// the server certificate afterwards if needed.
    pub(crate) fn get_quic_connection_bootstrap_meta(&self) -> ConnectionMeta {
        let (name, host) = self.main_hosts.iter().next().unwrap();

        ConnectionMeta {
            sni: name.clone(),
            protocol: Protocol::Http3,
            channel: Channel::Tunnel,
            cert_chain: Default::default(), // quiche only accepts paths
            key: PrivateKeyDer::Pkcs8(rustls_pki_types::PrivatePkcs8KeyDer::from(vec![])), // quiche only accepts paths
            cert_chain_path: host.cert_chain_path.clone(),
            key_path: host.key_path.clone(),
            sni_auth_creds: None,
            boring: host.boring.clone(),
        }
    }

    pub(crate) fn select<'a, I>(&self, alpn: I, sni: String) -> Result<ConnectionMeta, String>
    where
        I: Iterator<Item = &'a [u8]> + Clone,
    {
        let parsed_alpn: Vec<_> = alpn
            .clone()
            .map(std::str::from_utf8)
            .filter_map(Result::ok)
            .filter_map(Protocol::from_alpn)
            .collect();
        if parsed_alpn.is_empty() && alpn.clone().peekable().peek().is_some() {
            return Err(format!(
                "None of advertised ALPNs successfully parsed: {:?}",
                alpn.map(utils::hex_dump).collect::<Vec<_>>()
            ));
        }

        let (protocol, channel, host, auth) = if let Some(h) = self.subscription_hosts.get(&sni) {
            (
                parsed_alpn
                    .iter()
                    .max()
                    .cloned()
                    .unwrap_or(DEFAULT_PROTOCOL),
                Channel::Subscription,
                h,
                None,
            )
        } else if let Some(h) = self.main_hosts.get(&sni) {
            (
                self.select_tunnel_channel_protocol(parsed_alpn.iter(), alpn)?,
                Channel::Tunnel,
                h,
                None,
            )
        } else if let Some(h) = self.reverse_proxy_hosts.get(&sni) {
            match parsed_alpn
                .iter()
                .filter(|x| matches!(x, Protocol::Http1 | Protocol::Http3))
                .max()
                .cloned()
            {
                Some(x) => (x, Channel::ReverseProxy, h, None),
                None if alpn.clone().peekable().peek().is_none() => {
                    (DEFAULT_PROTOCOL, Channel::ReverseProxy, h, None)
                }
                None => {
                    return Err(format!(
                        "Unexpected ALPN on reverse proxy connection {:?}",
                        alpn.map(utils::hex_dump).collect::<Vec<_>>()
                    ))
                }
            }
        } else if let Some(h) = self.ping_hosts.get(&sni) {
            (
                parsed_alpn
                    .iter()
                    .max()
                    .cloned()
                    .unwrap_or(DEFAULT_PROTOCOL),
                Channel::Ping,
                h,
                None,
            )
        } else if let Some(h) = self.speedtest_hosts.get(&sni) {
            (
                parsed_alpn
                    .iter()
                    .max()
                    .cloned()
                    .unwrap_or(DEFAULT_PROTOCOL),
                Channel::Speedtest,
                h,
                None,
            )
        } else if let Some((host, auth_creds)) = sni
            .split_once('.')
            .and_then(|(a, b)| self.main_hosts.get(b).zip(Some(a)))
        {
            (
                self.select_tunnel_channel_protocol(parsed_alpn.iter(), alpn)?,
                Channel::Tunnel,
                host,
                Some(String::from(auth_creds)),
            )
        } else if let Some(main_hostname) = self.allowed_sni_to_main_host.get(&sni) {
            let host = self.main_hosts.get(main_hostname).unwrap();
            (
                self.select_tunnel_channel_protocol(parsed_alpn.iter(), alpn)?,
                Channel::Tunnel,
                host,
                None,
            )
        } else {
            return Err(format!("Unexpected SNI {}", sni));
        };

        Ok(ConnectionMeta {
            sni,
            protocol,
            channel,
            cert_chain: host.cert_chain.clone(),
            key: host.key.clone_key(),
            cert_chain_path: host.cert_chain_path.clone(),
            key_path: host.key_path.clone(),
            sni_auth_creds: auth,
            boring: host.boring.clone(),
        })
    }

    fn select_tunnel_channel_protocol<'i1, 'i2, I1, I2>(
        &self,
        parsed_advertised_alpn: I1,
        advertised_alpn: I2,
    ) -> Result<Protocol, String>
    where
        I1: Iterator<Item = &'i1 Protocol>,
        I2: Iterator<Item = &'i2 [u8]> + Clone,
    {
        match parsed_advertised_alpn
            .filter(|x| self.tunnel_protocols.contains(x))
            .max()
            .cloned()
        {
            Some(x) => Ok(x),
            None if self.tunnel_protocols.contains(&DEFAULT_PROTOCOL)
                && advertised_alpn.clone().peekable().peek().is_none() =>
            {
                Ok(DEFAULT_PROTOCOL)
            }
            None => Err(format!(
                "Unexpected ALPN on reverse proxy connection {:?}",
                advertised_alpn.map(utils::hex_dump).collect::<Vec<_>>()
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::net_utils::Channel;
    use crate::settings::{
        Http1Settings, Http2Settings, ListenProtocolSettings, QuicSettings, ReverseProxySettings,
        Settings, TlsHostInfo, TlsHostsSettings,
    };
    use crate::subscription::SubscriptionSettings;
    use crate::tls_demultiplexer;
    use crate::tls_demultiplexer::{ConnectionMeta, Protocol};
    use std::net::ToSocketAddrs;
    use tls_demultiplexer::TlsDemux;

    fn dummy_reverse_proxy_settings() -> ReverseProxySettings {
        ReverseProxySettings {
            server_address: "0.0.0.0:0".to_socket_addrs().unwrap().next().unwrap(),
            path_mask: Default::default(),
            h3_backward_compatibility: Default::default(),
        }
    }

    fn listen_protocol_settings_as_str(x: &ListenProtocolSettings) -> String {
        x.http1
            .iter()
            .map(|_| "HTTP1")
            .chain(x.http2.iter().map(|_| "HTTP2"))
            .chain(x.quic.iter().map(|_| "QUIC"))
            .collect::<Vec<_>>()
            .join(", ")
    }

    fn make_tls_host(host: String) -> TlsHostInfo {
        TlsHostInfo {
            hostname: host,
            ..Default::default()
        }
    }

    fn settings_with_subscription() -> Settings {
        let mut settings = Settings::default();
        settings.subscription = Some(SubscriptionSettings {
            hostname: Some("vpn.example.com".to_string()),
            ..Default::default()
        });
        settings
    }

    fn check_protocol_selection(
        listen_protocols: ListenProtocolSettings,
        advertised_protocols: Vec<Protocol>,
    ) -> Result<ConnectionMeta, String> {
        const TEST_HOST: &str = "httpbin.agrd.dev";

        let mut settings = Settings::default();
        settings.listen_protocols = listen_protocols;

        let mut tls_settings = TlsHostsSettings::default();
        tls_settings.main_hosts = vec![make_tls_host(TEST_HOST.to_string())];

        let demux = TlsDemux::new(&settings, &tls_settings).unwrap();
        demux.select(
            advertised_protocols
                .iter()
                .map(Protocol::as_alpn)
                .map(str::as_bytes),
            TEST_HOST.to_string(),
        )
    }

    #[test]
    fn no_matching_tunnel_protocols() {
        struct Sample {
            listen_protocols: ListenProtocolSettings,
            advertised_protocols: Vec<Protocol>,
        }

        let test_samples = vec![
            Sample {
                listen_protocols: Default::default(),
                advertised_protocols: vec![],
            },
            Sample {
                listen_protocols: Default::default(),
                advertised_protocols: vec![Protocol::Http1],
            },
            Sample {
                listen_protocols: ListenProtocolSettings {
                    http1: Some(Http1Settings::builder().build()),
                    ..Default::default()
                },
                advertised_protocols: vec![Protocol::Http2],
            },
            Sample {
                listen_protocols: ListenProtocolSettings {
                    http2: Some(Http2Settings::builder().build()),
                    ..Default::default()
                },
                advertised_protocols: vec![Protocol::Http1],
            },
            Sample {
                listen_protocols: ListenProtocolSettings {
                    http2: Some(Http2Settings::builder().build()),
                    quic: Some(QuicSettings::builder().build()),
                    ..Default::default()
                },
                advertised_protocols: vec![Protocol::Http1],
            },
        ];

        for sample in test_samples {
            check_protocol_selection(
                sample.listen_protocols.clone(),
                sample.advertised_protocols.clone(),
            )
            .expect_err(&format!(
                "{:?}",
                (
                    listen_protocol_settings_as_str(&sample.listen_protocols),
                    sample.advertised_protocols
                )
            ));
        }
    }

    #[test]
    fn tunnel_protocol_selection() {
        struct Sample {
            listen_protocols: ListenProtocolSettings,
            advertised_protocols: Vec<Protocol>,
            expected_selection: Protocol,
        }

        let test_samples = vec![
            Sample {
                listen_protocols: ListenProtocolSettings {
                    http1: Some(Http1Settings::builder().build()),
                    http2: Some(Http2Settings::builder().build()),
                    ..Default::default()
                },
                advertised_protocols: vec![],
                expected_selection: Protocol::Http1,
            },
            Sample {
                listen_protocols: ListenProtocolSettings {
                    http1: Some(Http1Settings::builder().build()),
                    http2: Some(Http2Settings::builder().build()),
                    ..Default::default()
                },
                advertised_protocols: vec![Protocol::Http1, Protocol::Http2],
                expected_selection: Protocol::Http2,
            },
        ];

        for sample in test_samples {
            let meta = check_protocol_selection(
                sample.listen_protocols.clone(),
                sample.advertised_protocols.clone(),
            )
            .unwrap_or_else(|_| {
                panic!(
                    "{:?}",
                    (
                        listen_protocol_settings_as_str(&sample.listen_protocols),
                        sample.advertised_protocols
                    )
                )
            });
            assert_eq!(sample.expected_selection, meta.protocol);
        }
    }

    #[test]
    fn reverse_proxy_protocol_selection() {
        const TEST_HOST: &str = "httpbin.agrd.dev";

        let mut settings = Settings::default();
        settings.reverse_proxy = Some(dummy_reverse_proxy_settings());

        let mut tls_settings = TlsHostsSettings::default();
        tls_settings.reverse_proxy_hosts = vec![make_tls_host(TEST_HOST.to_string())];

        let demux = TlsDemux::new(&settings, &tls_settings).unwrap();

        let meta = demux
            .select(
                [Protocol::Http1.as_alpn().as_bytes()].into_iter(),
                TEST_HOST.to_string(),
            )
            .unwrap();
        assert_eq!(meta.protocol, Protocol::Http1);
        demux
            .select(
                [Protocol::Http2.as_alpn().as_bytes()].into_iter(),
                TEST_HOST.to_string(),
            )
            .unwrap_err();
        let meta = demux
            .select(
                [Protocol::Http3.as_alpn().as_bytes()].into_iter(),
                TEST_HOST.to_string(),
            )
            .unwrap();
        assert_eq!(meta.protocol, Protocol::Http3);
    }

    #[test]
    fn ping_protocol_selection() {
        const TEST_HOST: &str = "httpbin.agrd.dev";

        let mut tls_settings = TlsHostsSettings::default();
        tls_settings.ping_hosts = vec![make_tls_host(TEST_HOST.to_string())];

        let demux = TlsDemux::new(&Settings::default(), &tls_settings).unwrap();

        let meta = demux
            .select(
                [Protocol::Http1.as_alpn().as_bytes()].into_iter(),
                TEST_HOST.to_string(),
            )
            .unwrap();
        assert_eq!(meta.protocol, Protocol::Http1);
        let meta = demux
            .select(
                [Protocol::Http2.as_alpn().as_bytes()].into_iter(),
                TEST_HOST.to_string(),
            )
            .unwrap();
        assert_eq!(meta.protocol, Protocol::Http2);
        let meta = demux
            .select(
                [Protocol::Http3.as_alpn().as_bytes()].into_iter(),
                TEST_HOST.to_string(),
            )
            .unwrap();
        assert_eq!(meta.protocol, Protocol::Http3);
    }

    #[test]
    fn speedtest_protocol_selection() {
        const TEST_HOST: &str = "httpbin.agrd.dev";

        let mut tls_settings = TlsHostsSettings::default();
        tls_settings.speedtest_hosts = vec![make_tls_host(TEST_HOST.to_string())];

        let demux = TlsDemux::new(&Settings::default(), &tls_settings).unwrap();

        let meta = demux
            .select(
                [Protocol::Http1.as_alpn().as_bytes()].into_iter(),
                TEST_HOST.to_string(),
            )
            .unwrap();
        assert_eq!(meta.protocol, Protocol::Http1);
        let meta = demux
            .select(
                [Protocol::Http2.as_alpn().as_bytes()].into_iter(),
                TEST_HOST.to_string(),
            )
            .unwrap();
        assert_eq!(meta.protocol, Protocol::Http2);
        let meta = demux
            .select(
                [Protocol::Http3.as_alpn().as_bytes()].into_iter(),
                TEST_HOST.to_string(),
            )
            .unwrap();
        assert_eq!(meta.protocol, Protocol::Http3);
    }

    #[test]
    fn channel_selection() {
        struct Sample {
            sni: &'static str,
            expected_selection: Channel,
        }

        let test_samples = vec![
            Sample {
                sni: "tunnel",
                expected_selection: Channel::Tunnel,
            },
            Sample {
                sni: "ping",
                expected_selection: Channel::Ping,
            },
            Sample {
                sni: "speedtest",
                expected_selection: Channel::Speedtest,
            },
            Sample {
                sni: "reverse.proxy",
                expected_selection: Channel::ReverseProxy,
            },
        ];

        let mut settings = Settings::default();
        settings.reverse_proxy = Some(dummy_reverse_proxy_settings());

        let mut tls_settings = TlsHostsSettings::default();
        tls_settings.main_hosts = vec![make_tls_host("tunnel".to_string())];
        tls_settings.ping_hosts = vec![make_tls_host("ping".to_string())];
        tls_settings.speedtest_hosts = vec![make_tls_host("speedtest".to_string())];
        tls_settings.reverse_proxy_hosts = vec![make_tls_host("reverse.proxy".to_string())];

        let demux = TlsDemux::new(&settings, &tls_settings).unwrap();
        let advertised_alpn = [Protocol::Http1.as_alpn().as_bytes()].into_iter();

        for sample in test_samples {
            let meta = demux
                .select(advertised_alpn.clone(), sample.sni.to_string())
                .unwrap();
            assert_eq!(meta.channel, sample.expected_selection);
        }
    }

    #[test]
    fn sni_authentication() {
        const TUNNEL_HOST: &str = "endpoint";
        const CREDENTIALS: &str = "creds";

        let mut settings = Settings::default();
        settings.reverse_proxy = Some(dummy_reverse_proxy_settings());

        let mut tls_settings = TlsHostsSettings::default();
        tls_settings.main_hosts = vec![make_tls_host(TUNNEL_HOST.to_string())];
        tls_settings.ping_hosts = vec![make_tls_host(format!("ping.{TUNNEL_HOST}"))];
        tls_settings.speedtest_hosts = vec![make_tls_host(format!("speedtest.{TUNNEL_HOST}"))];
        tls_settings.reverse_proxy_hosts =
            vec![make_tls_host(format!("reverse.proxy.{TUNNEL_HOST}"))];

        let demux = TlsDemux::new(&settings, &tls_settings).unwrap();
        let advertised_alpn = [Protocol::Http1.as_alpn().as_bytes()].into_iter();
        let meta = demux
            .select(
                advertised_alpn.clone(),
                format!("{CREDENTIALS}.{TUNNEL_HOST}"),
            )
            .unwrap();
        assert_eq!(meta.channel, Channel::Tunnel);
        assert_eq!(meta.sni_auth_creds.as_deref(), Some(CREDENTIALS));
    }

    #[test]
    fn reverse_proxy_set_up_without_hosts() {
        let mut settings = Settings::default();
        settings.reverse_proxy = Some(dummy_reverse_proxy_settings());

        TlsDemux::new(&settings, &TlsHostsSettings::default())
            .map(|_| ())
            .unwrap();
    }

    #[test]
    fn alternative_sni_support() {
        const MAIN_HOST: &str = "example.org";
        const ALT_SNI_1: &str = "fake1.com";
        const ALT_SNI_2: &str = "fake2.net";

        let settings = Settings::default();

        let mut tls_settings = TlsHostsSettings::default();
        tls_settings.main_hosts = vec![TlsHostInfo {
            hostname: MAIN_HOST.to_string(),
            allowed_sni: vec![ALT_SNI_1.to_string(), ALT_SNI_2.to_string()],
            ..Default::default()
        }];

        let demux = TlsDemux::new(&settings, &tls_settings).unwrap();
        let advertised_alpn = [Protocol::Http1.as_alpn().as_bytes()].into_iter();

        let meta = demux
            .select(advertised_alpn.clone(), MAIN_HOST.to_string())
            .unwrap();
        assert_eq!(meta.channel, Channel::Tunnel);
        assert_eq!(meta.sni, MAIN_HOST);

        let meta = demux
            .select(advertised_alpn.clone(), ALT_SNI_1.to_string())
            .unwrap();
        assert_eq!(meta.channel, Channel::Tunnel);
        assert_eq!(meta.sni, ALT_SNI_1);
        assert!(meta.sni_auth_creds.is_none());

        let meta = demux
            .select(advertised_alpn.clone(), ALT_SNI_2.to_string())
            .unwrap();
        assert_eq!(meta.channel, Channel::Tunnel);
        assert_eq!(meta.sni, ALT_SNI_2);
        assert!(meta.sni_auth_creds.is_none());

        demux
            .select(advertised_alpn.clone(), "unknown.sni".to_string())
            .expect_err("Unknown SNI should fail");
    }

    #[test]
    fn subscription_protocol_selection() {
        const TEST_HOST: &str = "sub.localhost";

        let mut tls_settings = TlsHostsSettings::default();
        tls_settings.subscription_hosts = vec![make_tls_host(TEST_HOST.to_string())];

        let settings = settings_with_subscription();
        let demux = TlsDemux::new(&settings, &tls_settings).unwrap();

        let meta = demux
            .select(
                [Protocol::Http1.as_alpn().as_bytes()].into_iter(),
                TEST_HOST.to_string(),
            )
            .unwrap();
        assert_eq!(meta.protocol, Protocol::Http1);
        let meta = demux
            .select(
                [Protocol::Http2.as_alpn().as_bytes()].into_iter(),
                TEST_HOST.to_string(),
            )
            .unwrap();
        assert_eq!(meta.protocol, Protocol::Http2);
        let meta = demux
            .select(
                [Protocol::Http3.as_alpn().as_bytes()].into_iter(),
                TEST_HOST.to_string(),
            )
            .unwrap();
        assert_eq!(meta.protocol, Protocol::Http3);
        let meta = demux
            .select(std::iter::empty(), TEST_HOST.to_string())
            .unwrap();
        assert_eq!(meta.protocol, Protocol::Http1);
    }

    #[test]
    fn subscription_channel_selection_has_priority_over_credential_split() {
        const MAIN_HOST: &str = "endpoint";
        const SUBSCRIPTION_HOST: &str = "sub.endpoint";
        const CREDENTIALS: &str = "creds";

        let mut tls_settings = TlsHostsSettings::default();
        tls_settings.main_hosts = vec![make_tls_host(MAIN_HOST.to_string())];
        tls_settings.subscription_hosts = vec![make_tls_host(SUBSCRIPTION_HOST.to_string())];

        let settings = settings_with_subscription();
        let demux = TlsDemux::new(&settings, &tls_settings).unwrap();
        let advertised_alpn = [Protocol::Http1.as_alpn().as_bytes()].into_iter();

        let meta = demux
            .select(advertised_alpn.clone(), SUBSCRIPTION_HOST.to_string())
            .unwrap();
        assert_eq!(meta.channel, Channel::Subscription);
        assert!(meta.sni_auth_creds.is_none());

        let meta = demux
            .select(
                advertised_alpn.clone(),
                format!("{CREDENTIALS}.{MAIN_HOST}"),
            )
            .unwrap();
        assert_eq!(meta.channel, Channel::Tunnel);
        assert_eq!(meta.sni_auth_creds.as_deref(), Some(CREDENTIALS));
    }

    #[test]
    fn subscription_hosts_not_served_without_subscription_section() {
        let mut tls_settings = TlsHostsSettings::default();
        tls_settings.subscription_hosts = vec![make_tls_host("sub.localhost".to_string())];

        let demux = TlsDemux::new(&Settings::default(), &tls_settings).unwrap();

        demux
            .select(
                [Protocol::Http1.as_alpn().as_bytes()].into_iter(),
                "sub.localhost".to_string(),
            )
            .expect_err("subscription host must not be served without [subscription]");
    }
}
