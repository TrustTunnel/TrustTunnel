use std::collections::HashSet;
use std::fmt::{Debug, Formatter};
use std::io;
use std::io::ErrorKind;
use std::net::{Ipv4Addr, SocketAddr, ToSocketAddrs};
use std::path::Path;
use std::time::Duration;

use crate::{authentication, rules, utils};
use authentication::registry_based::Client;
#[cfg(feature = "rt_doc")]
use macros::{Getter, RuntimeDoc};
use serde::{Deserialize, Serialize};
use toml_edit::{Document, Item};

pub type Socks5BuilderResult<T> = Result<T, Socks5Error>;

pub enum ValidationError {
    /// [`Settings.listen_address`] is not set
    ListenAddressNotSet,
    /// Invalid [`TlsHostsSettings.main_hosts`]
    MainTlsHostInfo(String),
    /// Invalid [`TlsHostsSettings.ping_hosts`]
    PingTlsHostInfo(String),
    /// Invalid [`TlsHostsSettings.speedtest_hosts`]
    SpeedTlsHostInfo(String),
    /// Invalid [`Settings.reverse_proxy`]
    ReverseProxy(String),
    /// Invalid [`Settings.listen_protocols`]
    ListenProtocols(String),
    /// Invalid rules file
    RulesFile(String),
}

impl Debug for ValidationError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ListenAddressNotSet => write!(f, "Listen address is not set"),
            Self::MainTlsHostInfo(x) => write!(f, "Invalid main TLS hosts: {}", x),
            Self::PingTlsHostInfo(x) => write!(f, "Invalid ping TLS hosts: {}", x),
            Self::SpeedTlsHostInfo(x) => write!(f, "Invalid speedtest TLS hosts: {}", x),
            Self::ReverseProxy(x) => write!(f, "Invalid reverse proxy settings: {}", x),
            Self::ListenProtocols(x) => write!(f, "Invalid listen protocols settings: {}", x),
            Self::RulesFile(x) => write!(f, "Invalid rules file: {}", x),
        }
    }
}

pub enum Socks5Error {
    /// [`Socks5ForwarderSettings.address`] is not set
    AddressNotSet,
}

impl Debug for Socks5Error {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::AddressNotSet => write!(f, "Server address is not set"),
        }
    }
}

#[derive(Serialize, Deserialize)]
#[cfg_attr(feature = "rt_doc", derive(Getter, RuntimeDoc))]
pub struct Settings {
    /// The address to listen on
    #[serde(default = "Settings::default_listen_address")]
    pub(crate) listen_address: SocketAddr,
    /// Whether IPv6 connections can be routed or rejected with unreachable status
    #[serde(default = "Settings::default_ipv6_available")]
    pub(crate) ipv6_available: bool,
    /// Whether connections to private network of the endpoint are allowed
    #[serde(default = "Settings::default_allow_private_network_connections")]
    pub(crate) allow_private_network_connections: bool,
    /// Timeout of an incoming TLS handshake
    #[serde(default = "Settings::default_tls_handshake_timeout")]
    #[serde(rename = "tls_handshake_timeout_secs")]
    #[serde(
        deserialize_with = "deserialize_duration_secs",
        serialize_with = "serialize_duration_secs"
    )]
    pub(crate) tls_handshake_timeout: Duration,
    /// Timeout of a client listener
    #[serde(default = "Settings::default_client_listener_timeout")]
    #[serde(rename = "client_listener_timeout_secs")]
    #[serde(
        deserialize_with = "deserialize_duration_secs",
        serialize_with = "serialize_duration_secs"
    )]
    pub(crate) client_listener_timeout: Duration,
    /// Timeout of outgoing connection establishment.
    /// For example, it is related to client's connection requests.
    #[serde(default = "Settings::default_connection_establishment_timeout")]
    #[serde(rename = "connection_establishment_timeout_secs")]
    #[serde(
        deserialize_with = "deserialize_duration_secs",
        serialize_with = "serialize_duration_secs"
    )]
    pub(crate) connection_establishment_timeout: Duration,
    /// Idle timeout of tunneled TCP connections
    #[serde(default = "Settings::default_tcp_connections_timeout")]
    #[serde(rename = "tcp_connections_timeout_secs")]
    #[serde(
        deserialize_with = "deserialize_duration_secs",
        serialize_with = "serialize_duration_secs"
    )]
    pub(crate) tcp_connections_timeout: Duration,
    /// Timeout of tunneled UDP "connections"
    #[serde(default = "Settings::default_udp_connections_timeout")]
    #[serde(rename = "udp_connections_timeout_secs")]
    #[serde(
        deserialize_with = "deserialize_duration_secs",
        serialize_with = "serialize_duration_secs"
    )]
    pub(crate) udp_connections_timeout: Duration,
    /// The set of connection forwarder settings
    #[serde(default)]
    pub(crate) forward_protocol: ForwardProtocolSettings,
    /// The set of enabled client listener codecs
    pub(crate) listen_protocols: ListenProtocolSettings,
    // TODO (ayakushin): fix docs
    /// The client authenticator.
    ///
    /// If [forward_protocol](Settings.forward_protocol) is set to
    /// [SOCKS5](ForwardProtocolSettings::Socks5), the endpoint will advertise authentication
    /// to the SOCKS5 server even if the [`Settings::authenticator`] is [`None`].
    ///
    /// In case the [`Settings`] are deserialized from a file, the authenticator
    /// is specified as the `credentials_file` field, which contains the path to the TOML
    /// file in the form shown below, and the resulting type of the authenticator is
    /// [`RegistryBasedAuthenticator`].
    ///
    /// The credentials file format:
    ///
    /// ```text
    /// [[client]]
    /// username = "a"
    /// password = "b"
    ///
    /// [[client]]
    /// ...
    /// ```
    #[serde(default)]
    #[serde(skip_serializing)]
    #[serde(rename(deserialize = "credentials_file"))]
    #[serde(deserialize_with = "deserialize_clients")]
    pub(crate) clients: Vec<Client>,
    /// The reverse proxy settings.
    /// With this one set up the endpoint does TLS termination on such connections and
    /// translates HTTP/x traffic into HTTP/1.1 protocol towards the server and back
    /// into original HTTP/x towards the client. Like this:
    ///
    /// ```(client) TLS(HTTP/x) <--(endpoint)--> (server) HTTP/1.1```
    ///
    /// The translated HTTP/1.1 requests have the custom header `X-Original-Protocol`
    /// appended. For now, its value can be either `HTTP1`, or `HTTP3`.
    /// TLS hosts for the reverse proxy channel are configured through [`TlsHostsSettings`].
    pub(crate) reverse_proxy: Option<ReverseProxySettings>,
    /// The ICMP forwarding settings.
    /// Setting up this feature requires superuser rights on some systems.
    pub(crate) icmp: Option<IcmpSettings>,
    /// The metrics gathering request handler settings
    pub(crate) metrics: Option<MetricsSettings>,
    /// Path to the rules file for connection filtering.
    /// If not specified or file doesn't exist, all connections are allowed by default.
    #[serde(default)]
    #[serde(skip_serializing)]
    #[serde(rename(deserialize = "rules_file"))]
    #[serde(deserialize_with = "deserialize_rules")]
    pub(crate) rules_engine: Option<rules::RulesEngine>,

    /// Whether an instance was built through a [`SettingsBuilder`].
    /// This flag is a workaround for absence of the ability to validate
    /// the deserialized structure.
    /// https://github.com/serde-rs/serde/issues/642
    #[serde(skip)]
    built: bool,
}

#[derive(Default, Serialize, Deserialize)]
#[cfg_attr(feature = "rt_doc", derive(RuntimeDoc))]
pub struct TlsHostInfo {
    /// Used as a key for selecting a certificate chain in TLS handshake.
    /// MUST be unique.
    pub hostname: String,
    /// Path to a file containing the certificate chain.
    /// MUST remain valid until [`crate::core::Core::listen()`] or
    /// [`crate::core::Core::listen_async()`] is running, or
    /// until the next [`crate::core::Core::reload_tls_hosts_settings()`] call.
    #[serde(deserialize_with = "deserialize_file_path")]
    pub cert_chain_path: String,
    /// Path to a file containing the private key.
    /// May be equal to `cert_chain_path` if it contains both of them.
    /// MUST remain valid until [`crate::core::Core::listen()`] or
    /// [`crate::core::Core::listen_async()`] is running, or
    /// until the next [`crate::core::Core::reload_tls_hosts_settings()`] call.
    #[serde(deserialize_with = "deserialize_file_path")]
    pub private_key_path: String,
}

#[derive(Serialize, Deserialize)]
#[cfg_attr(test, derive(Default))]
#[cfg_attr(feature = "rt_doc", derive(RuntimeDoc))]
pub struct TlsHostsSettings {
    /// Еhe main TLS hosts.
    /// Used for traffic tunneling and service requests handling.
    pub(crate) main_hosts: Vec<TlsHostInfo>,
    /// The TLS hosts for HTTPS pinging.
    /// With this one set up the endpoint responds with `200 OK` to HTTPS `GET` requests
    /// to the specified domains.
    #[serde(default)]
    pub(crate) ping_hosts: Vec<TlsHostInfo>,
    /// The TLS hosts for speed testing.
    /// With this one set up the endpoint accepts connections to the specified hosts and
    /// handles HTTP requests in the following way:
    ///     * `GET` requests with `/Nmb.bin` path (where `N` is 1 to 100, e.g. `/100mb.bin`)
    ///       are considered as download speedtest transferring `N` megabytes to a client
    ///     * `POST` requests with `/upload.html` path and `Content-Length: N`
    ///       are considered as upload speedtest receiving `N` bytes from a client,
    ///       where `N` is up to 120 * 1024 * 1024 bytes
    #[serde(default)]
    pub(crate) speedtest_hosts: Vec<TlsHostInfo>,
    /// The TLS hosts for the connections must be forwarded to the reverse proxy
    /// (see [`Settings::reverse_proxy`]).
    /// Only makes sense if the reverse proxy is set up, otherwise it is ignored.
    #[serde(default)]
    pub(crate) reverse_proxy_hosts: Vec<TlsHostInfo>,

    /// Whether an instance was built through a [`TlsSettingsBuilder`].
    /// This flag is a workaround for absence of the ability to validate
    /// the deserialized structure.
    /// https://github.com/serde-rs/serde/issues/642
    #[serde(skip)]
    built: bool,
}

#[derive(Serialize, Deserialize)]
#[cfg_attr(feature = "rt_doc", derive(RuntimeDoc))]
pub struct ReverseProxySettings {
    /// The origin server address
    pub(crate) server_address: SocketAddr,
    /// Connections to [the main hosts](TlsHostsSettings.main_hosts) with
    /// paths starting with this mask are routed to the reverse proxy server.
    /// MUST start with slash.
    pub(crate) path_mask: String,
    /// With this one set to `true` the endpoint overrides the HTTP method while
    /// translating an HTTP3 request to HTTP1 in case the request has the `GET` method
    /// and its path is `/` or matches [`ReverseProxySettings.path_mask`]
    #[serde(default)]
    pub(crate) h3_backward_compatibility: bool,
}

/// The set of connection forwarder settings
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "rt_doc", derive(RuntimeDoc))]
pub enum ForwardProtocolSettings {
    /// A direct forwarder routes a connection directly to its target host
    Direct(DirectForwarderSettings),
    /// A SOCKS5 forwarder routes a connection though a SOCKS5 proxy
    Socks5(Socks5ForwarderSettings),
}

#[derive(Serialize, Deserialize)]
pub struct DirectForwarderSettings {}

#[derive(Serialize, Deserialize)]
#[cfg_attr(feature = "rt_doc", derive(Getter, RuntimeDoc))]
pub struct Socks5ForwarderSettings {
    /// The address of a proxy
    pub(crate) address: SocketAddr,
    /// Whether the extended authentication is enabled
    #[serde(default)]
    pub(crate) extended_auth: bool,
}

pub struct Socks5ForwarderSettingsBuilder {
    settings: Socks5ForwarderSettings,
}

/// The set of enabled client listener codecs
#[derive(Clone, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "rt_doc", derive(RuntimeDoc))]
pub struct ListenProtocolSettings {
    /// HTTP/1.1 listener settings
    #[serde(default)]
    pub http1: Option<Http1Settings>,
    /// HTTP/2 listener settings
    #[serde(default)]
    pub http2: Option<Http2Settings>,
    #[serde(default)]
    /// QUIC / HTTP/3 listener settings
    pub quic: Option<QuicSettings>,
}

/// The ICMP forwarding settings.
/// Setting up this feature requires superuser rights on some systems.
#[derive(Serialize, Deserialize)]
#[cfg_attr(feature = "rt_doc", derive(Getter, RuntimeDoc))]
pub struct IcmpSettings {
    /// The name of a network interface to bind the outbound ICMP socket to
    #[serde(default = "IcmpSettings::default_interface_name")]
    pub(crate) interface_name: String,
    /// Timeout of tunneled ICMP requests
    #[serde(default = "IcmpSettings::default_request_timeout")]
    #[serde(rename = "request_timeout_secs")]
    #[serde(
        deserialize_with = "deserialize_duration_secs",
        serialize_with = "serialize_duration_secs"
    )]
    pub(crate) request_timeout: Duration,
    /// The capacity of the ICMP multiplexer received messages queue.
    /// Decreasing it may cause packet dropping in case the multiplexer cannot keep up the pace.
    /// Increasing it may lead to high memory consumption.
    /// Each client has its own queue.
    #[serde(default = "IcmpSettings::default_message_queue_capacity")]
    pub(crate) recv_message_queue_capacity: usize,
}

/// The metrics gathering request handler settings
#[derive(Serialize, Deserialize)]
#[cfg_attr(feature = "rt_doc", derive(Getter, RuntimeDoc))]
pub struct MetricsSettings {
    /// The address to listen on for settings export requests
    #[serde(default = "MetricsSettings::default_listen_address")]
    pub(crate) address: SocketAddr,
    /// Timeout of a metrics request
    #[serde(default = "MetricsSettings::default_request_timeout")]
    #[serde(rename = "request_timeout_secs")]
    #[serde(
        deserialize_with = "deserialize_duration_secs",
        serialize_with = "serialize_duration_secs"
    )]
    pub(crate) request_timeout: Duration,
}

/// The set of HTTP/1.1 listener codec settings
#[derive(Serialize, Deserialize, Clone)]
#[cfg_attr(feature = "rt_doc", derive(Getter, RuntimeDoc))]
pub struct Http1Settings {
    /// Buffer size for outgoing traffic
    #[serde(default = "Http1Settings::default_upload_buffer_size")]
    pub(crate) upload_buffer_size: usize,
}

/// The set of HTTP/2 listener codec settings
#[derive(Serialize, Deserialize, Clone)]
#[cfg_attr(feature = "rt_doc", derive(Getter, RuntimeDoc))]
pub struct Http2Settings {
    /// The initial window size (in octets) for connection-level flow control for received data
    #[serde(default = "Http2Settings::default_initial_connection_window_size")]
    pub(crate) initial_connection_window_size: u32,
    /// The initial window size (in octets) for stream-level flow control for received data
    #[serde(default = "Http2Settings::default_initial_stream_window_size")]
    pub(crate) initial_stream_window_size: u32,
    /// The number of streams that the sender permits the receiver to create
    #[serde(default = "Http2Settings::default_max_concurrent_streams")]
    pub(crate) max_concurrent_streams: u32,
    /// The size (in octets) of the largest HTTP/2 frame payload that we are able to accept
    #[serde(default = "Http2Settings::default_max_frame_size")]
    pub(crate) max_frame_size: u32,
    /// The max size of received header frames
    #[serde(default = "Http2Settings::default_header_table_size")]
    pub(crate) header_table_size: u32,
}

/// The set of QUIC listener codec settings
#[derive(Serialize, Deserialize, Clone)]
#[cfg_attr(feature = "rt_doc", derive(Getter, RuntimeDoc))]
pub struct QuicSettings {
    /// The size of UDP payloads that the endpoint is willing to receive. UDP datagrams with
    /// payloads larger than this limit are not likely to be processed.
    #[serde(default = "QuicSettings::default_recv_udp_payload_size")]
    pub(crate) recv_udp_payload_size: usize,
    /// The size of UDP payloads that the endpoint is willing to send
    #[serde(default = "QuicSettings::default_send_udp_payload_size")]
    pub(crate) send_udp_payload_size: usize,
    /// The initial value for the maximum amount of data that can be sent on the connection
    #[serde(default = "QuicSettings::default_initial_max_data")]
    pub(crate) initial_max_data: u64,
    /// The initial flow control limit for locally initiated bidirectional streams
    #[serde(default = "QuicSettings::default_initial_max_stream_data_bidi_local")]
    pub(crate) initial_max_stream_data_bidi_local: u64,
    /// The initial flow control limit for peer-initiated bidirectional streams
    #[serde(default = "QuicSettings::default_initial_max_stream_data_bidi_remote")]
    pub(crate) initial_max_stream_data_bidi_remote: u64,
    /// The initial flow control limit for unidirectional streams
    #[serde(default = "QuicSettings::default_initial_max_stream_data_uni")]
    pub(crate) initial_max_stream_data_uni: u64,
    /// The initial maximum number of bidirectional streams the endpoint that receives this
    /// transport parameter is permitted to initiate
    #[serde(default = "QuicSettings::default_initial_max_streams_bidi")]
    pub(crate) initial_max_streams_bidi: u64,
    /// The initial maximum number of unidirectional streams the endpoint that receives this
    /// transport parameter is permitted to initiate
    #[serde(default = "QuicSettings::default_initial_max_streams_uni")]
    pub(crate) initial_max_streams_uni: u64,
    /// The maximum size of the connection window
    #[serde(default = "QuicSettings::default_max_connection_window")]
    pub(crate) max_connection_window: u64,
    /// The maximum size of the stream window
    #[serde(default = "QuicSettings::default_max_stream_window")]
    pub(crate) max_stream_window: u64,
    /// Disable active connection migration on the address being used during the handshake
    #[serde(default = "QuicSettings::default_disable_active_migration")]
    pub(crate) disable_active_migration: bool,
    /// Enable sending or receiving early data
    #[serde(default = "QuicSettings::default_enable_early_data")]
    pub(crate) enable_early_data: bool,
    /// The capacity of the QUIC multiplexer message queue.
    /// Decreasing it may cause packet dropping in case the multiplexer cannot keep up the pace.
    /// Increasing it may lead to high memory consumption.
    // @todo: separate values for incoming and outgoing?
    #[serde(default = "QuicSettings::default_message_queue_capacity")]
    pub(crate) message_queue_capacity: usize,
}

pub struct SettingsBuilder {
    settings: Settings,
}

pub struct TlsSettingsBuilder {
    settings: TlsHostsSettings,
}

pub struct Http1SettingsBuilder {
    settings: Http1Settings,
}

pub struct Http2SettingsBuilder {
    settings: Http2Settings,
}

pub struct QuicSettingsBuilder {
    settings: QuicSettings,
}

pub struct ReverseProxySettingsBuilder {
    settings: ReverseProxySettings,
}

pub struct IcmpSettingsBuilder {
    settings: IcmpSettings,
}

pub struct MetricsSettingsBuilder {
    settings: MetricsSettings,
}

impl Settings {
    pub fn builder() -> SettingsBuilder {
        SettingsBuilder::new()
    }

    pub(crate) fn is_built(&self) -> bool {
        self.built
    }

    pub(crate) fn validate(&self) -> Result<(), ValidationError> {
        if self.listen_address.ip().is_unspecified() && self.listen_address.port() == 0 {
            return Err(ValidationError::ListenAddressNotSet);
        }

        self.reverse_proxy
            .as_ref()
            .map(ReverseProxySettings::validate)
            .transpose()?;

        if self.listen_protocols.http1.is_none()
            && self.listen_protocols.http2.is_none()
            && self.listen_protocols.quic.is_none()
        {
            return Err(ValidationError::ListenProtocols("Not set".into()));
        }

        Ok(())
    }

    pub fn default_listen_address() -> SocketAddr {
        SocketAddr::from((Ipv4Addr::UNSPECIFIED, 443))
    }

    pub fn default_ipv6_available() -> bool {
        true
    }

    pub fn default_allow_private_network_connections() -> bool {
        false
    }

    pub fn default_tls_handshake_timeout() -> Duration {
        Duration::from_secs(10)
    }

    pub fn default_client_listener_timeout() -> Duration {
        Duration::from_secs(10 * 60)
    }

    pub fn default_connection_establishment_timeout() -> Duration {
        Duration::from_secs(30)
    }

    pub fn default_tcp_connections_timeout() -> Duration {
        Duration::from_secs(604800) // 1 week (match client tcpip module)
    }

    pub fn default_udp_connections_timeout() -> Duration {
        Duration::from_secs(300) // 5 minutes (match client tcpip module)
    }
}

#[cfg(test)]
impl Default for Settings {
    fn default() -> Self {
        Self {
            listen_address: SocketAddr::from((Ipv4Addr::UNSPECIFIED, 0)),
            ipv6_available: false,
            allow_private_network_connections: true,
            tls_handshake_timeout: Settings::default_tls_handshake_timeout(),
            client_listener_timeout: Settings::default_client_listener_timeout(),
            connection_establishment_timeout: Settings::default_connection_establishment_timeout(),
            tcp_connections_timeout: Settings::default_tcp_connections_timeout(),
            udp_connections_timeout: Settings::default_udp_connections_timeout(),
            forward_protocol: Default::default(),
            clients: Default::default(),
            listen_protocols: ListenProtocolSettings {
                http1: Some(Http1Settings::builder().build()),
                http2: Some(Http2Settings::builder().build()),
                quic: Some(QuicSettings::builder().build()),
            },
            reverse_proxy: None,
            icmp: None,
            metrics: Default::default(),
            rules_engine: Some(rules::RulesEngine::default_allow()),
            built: false,
        }
    }
}

impl TlsHostsSettings {
    pub fn builder() -> TlsSettingsBuilder {
        TlsSettingsBuilder::new()
    }

    pub(crate) fn is_built(&self) -> bool {
        self.built
    }

    fn validate_tls_hosts<'a, Iter>(
        hosts: Iter,
        mut unique_hosts: HashSet<&'a str>,
    ) -> Result<HashSet<&'a str>, String>
    where
        Iter: Iterator<Item = &'a TlsHostInfo>,
    {
        for h in hosts {
            utils::load_certs(&h.cert_chain_path).map_err(|e| {
                format!(
                    "Invalid cert chain: path='{}', error='{}'",
                    h.cert_chain_path, e
                )
            })?;

            utils::load_private_key(&h.private_key_path).map_err(|e| {
                format!("Invalid key: path='{}', error='{}'", h.private_key_path, e)
            })?;

            if !unique_hosts.insert(&h.hostname) {
                return Err(format!("Hostname must be unique: {}", h.hostname));
            }
        }

        Ok(unique_hosts)
    }

    pub(crate) fn validate(&self) -> Result<(), ValidationError> {
        if self.main_hosts.is_empty() {
            return Err(ValidationError::MainTlsHostInfo("Not set".to_string()));
        }

        let hosts = Self::validate_tls_hosts(self.main_hosts.iter(), HashSet::new())
            .map_err(ValidationError::MainTlsHostInfo)?;
        let hosts = Self::validate_tls_hosts(self.ping_hosts.iter(), hosts)
            .map_err(ValidationError::PingTlsHostInfo)?;
        let hosts = Self::validate_tls_hosts(self.speedtest_hosts.iter(), hosts)
            .map_err(ValidationError::SpeedTlsHostInfo)?;
        Self::validate_tls_hosts(self.reverse_proxy_hosts.iter(), hosts)
            .map_err(ValidationError::ReverseProxy)?;

        Ok(())
    }
}

impl Socks5ForwarderSettings {
    pub fn builder() -> Socks5ForwarderSettingsBuilder {
        Socks5ForwarderSettingsBuilder::new()
    }
}

impl Http1Settings {
    pub fn builder() -> Http1SettingsBuilder {
        Http1SettingsBuilder::new()
    }

    pub fn default_upload_buffer_size() -> usize {
        32 * 1024
    }
}

impl Http2Settings {
    pub fn builder() -> Http2SettingsBuilder {
        Http2SettingsBuilder::new()
    }

    pub fn default_initial_connection_window_size() -> u32 {
        8 * 1024 * 1024
    }

    pub fn default_initial_stream_window_size() -> u32 {
        128 * 1024 // Chrome constant
    }

    pub fn default_max_concurrent_streams() -> u32 {
        1000 // Chrome constant
    }

    pub fn default_max_frame_size() -> u32 {
        1 << 14 // Firefox constant
    }

    pub fn default_header_table_size() -> u32 {
        65536
    }
}

impl QuicSettings {
    pub fn builder() -> QuicSettingsBuilder {
        QuicSettingsBuilder::new()
    }

    pub fn default_recv_udp_payload_size() -> usize {
        1350
    }

    pub fn default_send_udp_payload_size() -> usize {
        1350
    }

    pub fn default_initial_max_data() -> u64 {
        100 * 1024 * 1024
    }

    pub fn default_initial_max_stream_data_bidi_local() -> u64 {
        1024 * 1024
    }

    pub fn default_initial_max_stream_data_bidi_remote() -> u64 {
        1024 * 1024
    }

    pub fn default_initial_max_stream_data_uni() -> u64 {
        1024 * 1024
    }

    pub fn default_initial_max_streams_bidi() -> u64 {
        4 * 1024
    }

    pub fn default_initial_max_streams_uni() -> u64 {
        4 * 1024
    }

    pub fn default_max_connection_window() -> u64 {
        24 * 1024 * 1024
    }

    pub fn default_max_stream_window() -> u64 {
        16 * 1024 * 1024
    }

    pub fn default_disable_active_migration() -> bool {
        true
    }

    pub fn default_enable_early_data() -> bool {
        true
    }

    pub fn default_message_queue_capacity() -> usize {
        4 * 1024
    }
}

impl ReverseProxySettings {
    pub fn builder() -> ReverseProxySettingsBuilder {
        ReverseProxySettingsBuilder::new()
    }

    pub fn validate(&self) -> Result<(), ValidationError> {
        if self.server_address.port() == 0 {
            return Err(ValidationError::ReverseProxy(
                "Server address is not set".to_string(),
            ));
        }

        if self.path_mask.is_empty() || !self.path_mask.starts_with('/') {
            return Err(ValidationError::ReverseProxy(format!(
                "Invalid path mask: {}",
                self.path_mask
            )));
        }

        Ok(())
    }
}

impl IcmpSettings {
    pub fn builder() -> IcmpSettingsBuilder {
        IcmpSettingsBuilder::new()
    }

    pub fn default_interface_name() -> String {
        if cfg!(target_os = "linux") {
            "eth0"
        } else {
            "en0"
        }
        .into()
    }

    pub fn default_request_timeout() -> Duration {
        Duration::from_secs(3)
    }

    pub fn default_message_queue_capacity() -> usize {
        256
    }
}

impl MetricsSettings {
    pub fn builder() -> MetricsSettingsBuilder {
        MetricsSettingsBuilder::new()
    }

    pub fn default_listen_address() -> SocketAddr {
        (Ipv4Addr::UNSPECIFIED, 1987).into()
    }

    pub fn default_request_timeout() -> Duration {
        Duration::from_secs(3)
    }
}

impl Default for MetricsSettings {
    fn default() -> Self {
        Self {
            address: MetricsSettings::default_listen_address(),
            request_timeout: MetricsSettings::default_request_timeout(),
        }
    }
}

impl SettingsBuilder {
    fn new() -> Self {
        Self {
            settings: Settings {
                listen_address: Settings::default_listen_address(),
                ipv6_available: Settings::default_ipv6_available(),
                allow_private_network_connections:
                    Settings::default_allow_private_network_connections(),
                tls_handshake_timeout: Settings::default_tls_handshake_timeout(),
                client_listener_timeout: Settings::default_client_listener_timeout(),
                connection_establishment_timeout:
                    Settings::default_connection_establishment_timeout(),
                tcp_connections_timeout: Settings::default_tcp_connections_timeout(),
                udp_connections_timeout: Settings::default_udp_connections_timeout(),
                forward_protocol: Default::default(),
                listen_protocols: Default::default(),
                clients: Default::default(),
                reverse_proxy: None,
                icmp: None,
                metrics: Default::default(),
                rules_engine: Some(rules::RulesEngine::default_allow()),
                built: true,
            },
        }
    }

    /// Finalize [`Settings`]
    pub fn build(self) -> Result<Settings, ValidationError> {
        self.settings.validate()?;

        Ok(self.settings)
    }

    /// Set the address to listen on
    pub fn listen_address<A: ToSocketAddrs>(mut self, addr: A) -> io::Result<Self> {
        self.settings.listen_address = addr
            .to_socket_addrs()?
            .next()
            .ok_or_else(|| io::Error::new(ErrorKind::Other, "Address is parsed to empty list"))?;
        Ok(self)
    }

    /// Set the reverse proxy settings.
    /// With this one set up the endpoint does TLS termination on such connections and
    /// translates HTTP/x traffic into HTTP/1.1 protocol towards the server and back
    /// into original HTTP/x towards the client. Like this:
    ///
    /// ```(client) TLS(HTTP/x) <--(endpoint)--> (server) HTTP/1.1```
    ///
    /// The translated HTTP/1.1 requests have the custom header `X-Original-Protocol`
    /// appended. For now, its value can be either `HTTP1`, or `HTTP3`.
    /// TLS hosts for the reverse proxy channel are configured through [`TlsHostsSettings`].
    pub fn reverse_proxy(mut self, settings: ReverseProxySettings) -> Self {
        self.settings.reverse_proxy = Some(settings);
        self
    }

    /// Set IPv6 availability
    pub fn ipv6_available(mut self, v: bool) -> Self {
        self.settings.ipv6_available = v;
        self
    }

    /// Allow/disallow connections to private network of the endpoint
    pub fn allow_private_network_connections(mut self, v: bool) -> Self {
        self.settings.allow_private_network_connections = v;
        self
    }

    /// Set timeout of TLS handshake
    pub fn tls_handshake_timeout(mut self, v: Duration) -> Self {
        self.settings.tls_handshake_timeout = v;
        self
    }

    /// Set timeout of client listener
    pub fn client_listener_timeout(mut self, v: Duration) -> Self {
        self.settings.client_listener_timeout = v;
        self
    }

    /// Set timeout of outgoing connection establishment.
    /// For example, it is related to client's connection requests.
    pub fn connection_establishment_timeout(mut self, v: Duration) -> Self {
        self.settings.connection_establishment_timeout = v;
        self
    }

    /// Set timeout of tunneled TCP connections
    pub fn tcp_connections_timeout(mut self, v: Duration) -> Self {
        self.settings.tcp_connections_timeout = v;
        self
    }

    /// Set timeout of tunneled UDP "connections"
    pub fn udp_connections_timeout(mut self, v: Duration) -> Self {
        self.settings.udp_connections_timeout = v;
        self
    }

    /// Set the forwarder codec settings
    pub fn forwarder_settings(mut self, settings: ForwardProtocolSettings) -> Self {
        self.settings.forward_protocol = settings;
        self
    }

    /// Set the listener codec settings
    pub fn listen_protocols(mut self, settings: ListenProtocolSettings) -> Self {
        self.settings.listen_protocols = settings;
        self
    }

    /// Set the client authenticator
    pub fn clients(mut self, x: Vec<Client>) -> Self {
        self.settings.clients = x;
        self
    }

    /// Set the ICMP forwarder settings
    pub fn icmp(mut self, x: IcmpSettings) -> Self {
        self.settings.icmp = Some(x);
        self
    }

    /// Set the metrics request listener settings
    pub fn metrics(mut self, x: MetricsSettings) -> Self {
        self.settings.metrics = Some(x);
        self
    }

    /// Set the rules engine for connection filtering
    pub fn rules_engine(mut self, x: rules::RulesEngine) -> Self {
        self.settings.rules_engine = Some(x);
        self
    }
}

impl TlsSettingsBuilder {
    fn new() -> Self {
        Self {
            settings: TlsHostsSettings {
                main_hosts: Default::default(),
                ping_hosts: Default::default(),
                speedtest_hosts: Default::default(),
                reverse_proxy_hosts: Default::default(),
                built: true,
            },
        }
    }

    /// Finalize [`TlsHostsSettings`]
    pub fn build(self) -> Result<TlsHostsSettings, ValidationError> {
        self.settings.validate()?;
        Ok(self.settings)
    }

    /// Set the main TLS hosts.
    /// Used for traffic tunneling and service requests handling.
    pub fn main_hosts(mut self, hosts: Vec<TlsHostInfo>) -> Self {
        self.settings.main_hosts = hosts;
        self
    }

    /// Set the TLS hosts for HTTPS pinging.
    /// With this one set up the endpoint responds with `200 OK` to HTTPS `GET` requests
    /// to the specified domains.
    pub fn ping_hosts(mut self, hosts: Vec<TlsHostInfo>) -> Self {
        self.settings.ping_hosts = hosts;
        self
    }

    /// Set the TLS hosts for speed testing.
    /// With this one set up the endpoint accepts connections to the specified hosts and
    /// handles HTTP requests in the following way:
    ///     * `GET` requests with `/Nmb.bin` path (where `N` is 1 to 100, e.g. `/100mb.bin`)
    ///       are considered as download speedtest transferring `N` megabytes to a client
    ///     * `POST` requests with `/upload.html` path and `Content-Length: N`
    ///       are considered as upload speedtest receiving `N` bytes from a client,
    ///       where `N` is up to 120 * 1024 * 1024 bytes
    pub fn speedtest_hosts(mut self, hosts: Vec<TlsHostInfo>) -> Self {
        self.settings.speedtest_hosts = hosts;
        self
    }

    /// The TLS hosts for the connections must be forwarded to the reverse proxy
    /// (see [`self::Settings::reverse_proxy`]). Only makes sense if the reverse proxy
    /// is set up, otherwise it is ignored.
    pub fn reverse_proxy_hosts(mut self, hosts: Vec<TlsHostInfo>) -> Self {
        self.settings.reverse_proxy_hosts = hosts;
        self
    }
}

impl Socks5ForwarderSettingsBuilder {
    fn new() -> Self {
        Self {
            settings: Socks5ForwarderSettings {
                address: SocketAddr::from((Ipv4Addr::UNSPECIFIED, 0)),
                extended_auth: false,
            },
        }
    }

    /// Finalize [`Socks5ForwarderSettings`]
    pub fn build(self) -> Socks5BuilderResult<Socks5ForwarderSettings> {
        if self.settings.address.ip().is_unspecified() {
            return Err(Socks5Error::AddressNotSet);
        }

        Ok(self.settings)
    }

    /// Set the SOCKS proxy address
    pub fn server_address<A: ToSocketAddrs>(mut self, v: A) -> io::Result<Self> {
        self.settings.address = v
            .to_socket_addrs()?
            .next()
            .ok_or_else(|| io::Error::new(ErrorKind::Other, "Address is parsed to empty list"))?;
        Ok(self)
    }

    /// Enable/disable extended authentication.
    /// See README for details.
    pub fn extended_auth(mut self, v: bool) -> Self {
        self.settings.extended_auth = v;
        self
    }
}

impl Http1SettingsBuilder {
    fn new() -> Self {
        Self {
            settings: Http1Settings {
                upload_buffer_size: Http1Settings::default_upload_buffer_size(),
            },
        }
    }

    /// Finalize [`Http1Settings`]
    pub fn build(self) -> Http1Settings {
        self.settings
    }
}

impl Http2SettingsBuilder {
    fn new() -> Self {
        Self {
            settings: Http2Settings {
                initial_connection_window_size:
                    Http2Settings::default_initial_connection_window_size(),
                initial_stream_window_size: Http2Settings::default_initial_stream_window_size(),
                max_concurrent_streams: Http2Settings::default_max_concurrent_streams(),
                max_frame_size: Http2Settings::default_max_frame_size(),
                header_table_size: Http2Settings::default_header_table_size(),
            },
        }
    }

    /// Finalize [`Http2Settings`]
    pub fn build(self) -> Http2Settings {
        self.settings
    }

    /// Set the initial window size (in octets) for connection-level flow control for received data
    pub fn initial_connection_window_size(mut self, v: u32) -> Self {
        self.settings.initial_connection_window_size = v;
        self
    }

    /// Set the initial window size (in octets) for stream-level flow control for received data
    pub fn initial_stream_window_size(mut self, v: u32) -> Self {
        self.settings.initial_stream_window_size = v;
        self
    }

    /// Set the maximum number of concurrent streams
    pub fn max_concurrent_streams(mut self, v: u32) -> Self {
        self.settings.max_concurrent_streams = v;
        self
    }

    /// Set the size (in octets) of the largest HTTP/2 frame payload that we are able to accept
    pub fn max_frame_size(mut self, v: u32) -> Self {
        self.settings.max_frame_size = v;
        self
    }

    /// Set the max size of received header frames
    pub fn header_table_size(mut self, v: u32) -> Self {
        self.settings.header_table_size = v;
        self
    }
}

impl QuicSettingsBuilder {
    fn new() -> Self {
        Self {
            settings: QuicSettings {
                recv_udp_payload_size: QuicSettings::default_recv_udp_payload_size(),
                send_udp_payload_size: QuicSettings::default_send_udp_payload_size(),
                initial_max_data: QuicSettings::default_initial_max_data(),
                initial_max_stream_data_bidi_local:
                    QuicSettings::default_initial_max_stream_data_bidi_local(),
                initial_max_stream_data_bidi_remote:
                    QuicSettings::default_initial_max_stream_data_bidi_remote(),
                initial_max_stream_data_uni: QuicSettings::default_initial_max_stream_data_uni(),
                initial_max_streams_bidi: QuicSettings::default_initial_max_streams_bidi(),
                initial_max_streams_uni: QuicSettings::default_initial_max_streams_uni(),
                max_connection_window: QuicSettings::default_max_connection_window(),
                max_stream_window: QuicSettings::default_max_stream_window(),
                disable_active_migration: QuicSettings::default_disable_active_migration(),
                enable_early_data: QuicSettings::default_enable_early_data(),
                message_queue_capacity: QuicSettings::default_message_queue_capacity(),
            },
        }
    }

    /// Finalize [`QuicSettings`]
    pub fn build(self) -> QuicSettings {
        self.settings
    }

    /// Set the `max_udp_payload_size transport` parameter
    pub fn recv_udp_payload_size(mut self, v: usize) -> Self {
        self.settings.recv_udp_payload_size = v;
        self
    }

    /// Set the maximum outgoing UDP payload size
    pub fn send_udp_payload_size(mut self, v: usize) -> Self {
        self.settings.send_udp_payload_size = v;
        self
    }

    /// Set the `initial_max_data` transport parameter
    pub fn initial_max_data(mut self, v: u64) -> Self {
        self.settings.initial_max_data = v;
        self
    }

    /// Set the `initial_max_stream_data_bidi_local` transport parameter
    pub fn max_stream_data_bidi_local(mut self, v: u64) -> Self {
        self.settings.initial_max_stream_data_bidi_local = v;
        self
    }

    /// Set the `initial_max_stream_data_bidi_remote` transport parameter
    pub fn max_stream_data_bidi_remote(mut self, v: u64) -> Self {
        self.settings.initial_max_stream_data_bidi_remote = v;
        self
    }

    /// Set the `initial_max_stream_data_uni` transport parameter
    pub fn max_stream_data_uni(mut self, v: u64) -> Self {
        self.settings.initial_max_stream_data_uni = v;
        self
    }

    /// Set the `initial_max_streams_bidi` transport parameter
    pub fn max_streams_bidi(mut self, v: u64) -> Self {
        self.settings.initial_max_streams_bidi = v;
        self
    }

    /// Set the `initial_max_streams_uni` transport parameter
    pub fn max_streams_uni(mut self, v: u64) -> Self {
        self.settings.initial_max_streams_uni = v;
        self
    }

    /// Set the maximum size of the connection window
    pub fn max_connection_window(mut self, v: u64) -> Self {
        self.settings.max_connection_window = v;
        self
    }

    /// Set the maximum size of the stream window
    pub fn max_stream_window(mut self, v: u64) -> Self {
        self.settings.max_stream_window = v;
        self
    }

    /// Set the `disable_active_migration` transport parameter
    pub fn disable_active_migration(mut self, v: bool) -> Self {
        self.settings.disable_active_migration = v;
        self
    }

    /// Enable receiving early data
    pub fn enable_early_data(mut self, v: bool) -> Self {
        self.settings.enable_early_data = v;
        self
    }

    /// Set the capacity of the QUIC multiplexer message queue
    pub fn message_queue_capacity(mut self, v: usize) -> Self {
        self.settings.message_queue_capacity = v;
        self
    }
}

impl ReverseProxySettingsBuilder {
    fn new() -> Self {
        Self {
            settings: ReverseProxySettings {
                server_address: (Ipv4Addr::UNSPECIFIED, 0).into(),
                path_mask: Default::default(),
                h3_backward_compatibility: false,
            },
        }
    }

    /// Finalize [`ReverseProxySettings`]
    pub fn build(self) -> Result<ReverseProxySettings, ValidationError> {
        self.settings.validate()?;
        Ok(self.settings)
    }

    /// Set the proxy server address
    pub fn server_address<A: ToSocketAddrs>(mut self, v: A) -> io::Result<Self> {
        self.settings.server_address = v
            .to_socket_addrs()?
            .next()
            .ok_or_else(|| io::Error::new(ErrorKind::Other, "Address is parsed to empty list"))?;
        Ok(self)
    }

    /// Connections to [the main hosts](TlsHostsSettings.main_hosts) with
    /// paths starting with this mask are routed to the reverse proxy server.
    /// MUST start with slash.
    pub fn path_mask(mut self, v: String) -> Self {
        self.settings.path_mask = v;
        self
    }

    /// With this one set to `true` the endpoint overrides the HTTP method while
    /// translating an HTTP3 request to HTTP1 in case the request has the `GET` method
    /// and its path is `/` or matches [`ReverseProxySettings.path_mask`]
    pub fn h3_backward_compatibility(mut self, v: bool) -> Self {
        self.settings.h3_backward_compatibility = v;
        self
    }
}

impl IcmpSettingsBuilder {
    fn new() -> Self {
        Self {
            settings: IcmpSettings {
                interface_name: Default::default(),
                request_timeout: IcmpSettings::default_request_timeout(),
                recv_message_queue_capacity: IcmpSettings::default_message_queue_capacity(),
            },
        }
    }

    /// Set the interface name to bind the socket to
    pub fn interface_name<S: ToString>(mut self, v: S) -> Self {
        self.settings.interface_name = v.to_string();
        self
    }

    /// Set the ICMP request timeout
    pub fn request_timeout(mut self, v: Duration) -> Self {
        self.settings.request_timeout = v;
        self
    }

    /// Set the capacity of the ICMP multiplexer received messages queue
    pub fn recv_message_queue_capacity(mut self, v: usize) -> Self {
        self.settings.recv_message_queue_capacity = v;
        self
    }

    /// Finalize [`IcmpSettings`]
    pub fn build(self) -> Result<IcmpSettings, ValidationError> {
        Ok(self.settings)
    }
}

impl MetricsSettingsBuilder {
    fn new() -> Self {
        Self {
            settings: Default::default(),
        }
    }

    /// Set the address to listen on for settings export requests
    pub fn listen_address<A: ToSocketAddrs>(mut self, addr: A) -> io::Result<Self> {
        self.settings.address = addr
            .to_socket_addrs()?
            .next()
            .ok_or_else(|| io::Error::new(ErrorKind::Other, "Address is parsed to empty list"))?;
        Ok(self)
    }

    /// Set the metrics request timeout
    pub fn request_timeout(mut self, v: Duration) -> Self {
        self.settings.request_timeout = v;
        self
    }

    /// Finalize [`MetricsSettings`]
    pub fn build(self) -> Result<MetricsSettings, ValidationError> {
        Ok(self.settings)
    }
}

impl Default for ForwardProtocolSettings {
    fn default() -> Self {
        ForwardProtocolSettings::Direct(DirectForwarderSettings {})
    }
}

fn validate_file_path(path: &str) -> io::Result<()> {
    match std::fs::metadata(Path::new(path))? {
        m if m.is_file() => Ok(()),
        _ => Err(io::Error::new(ErrorKind::Other, "Not a file")),
    }
}

fn deserialize_duration_secs<'de, D>(deserializer: D) -> Result<Duration, D::Error>
where
    D: serde::de::Deserializer<'de>,
{
    struct Visitor;

    impl<'de> serde::de::Visitor<'de> for Visitor {
        type Value = u64;

        fn expecting(&self, formatter: &mut Formatter) -> std::fmt::Result {
            write!(formatter, "an unsigned integer")
        }

        // toml parser library converts unsigned integers to signed
        fn visit_i64<E>(self, v: i64) -> Result<Self::Value, E>
        where
            E: serde::de::Error,
        {
            (v >= 0)
                .then_some(v as u64)
                .ok_or_else(|| E::invalid_type(serde::de::Unexpected::Signed(v), &Visitor {}))
        }
    }

    let x = deserializer.deserialize_u64(Visitor)?;
    Ok(Duration::from_secs(x))
}

fn serialize_duration_secs<S>(x: &Duration, serializer: S) -> Result<S::Ok, S::Error>
where
    S: serde::ser::Serializer,
{
    serializer.serialize_u64(x.as_secs())
}

fn deserialize_file_path<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: serde::de::Deserializer<'de>,
{
    struct Visitor;

    impl<'de> serde::de::Visitor<'de> for Visitor {
        type Value = String;

        fn expecting(&self, formatter: &mut Formatter) -> std::fmt::Result {
            write!(formatter, "a path to an existent accessible file")
        }

        fn visit_str<E>(self, v: &str) -> Result<Self::Value, E>
        where
            E: serde::de::Error,
        {
            validate_file_path(v).map(|_| v.to_string()).map_err(|e| {
                E::invalid_value(
                    serde::de::Unexpected::Other(&format!("path={} error={}", v, e)),
                    &Visitor {},
                )
            })
        }
    }

    deserializer.deserialize_str(Visitor)
}

fn deserialize_clients<'de, D>(deserializer: D) -> Result<Vec<Client>, D::Error>
where
    D: serde::de::Deserializer<'de>,
{
    let path = deserialize_file_path(deserializer)?;

    let content = std::fs::read_to_string(&path).map_err(|e| {
        serde::de::Error::invalid_value(
            serde::de::Unexpected::Other(&format!("Couldn't read file: path={} error={}", path, e)),
            &"A readable file",
        )
    })?;

    let clients: Document = content.parse().map_err(|e| {
        serde::de::Error::invalid_value(
            serde::de::Unexpected::Other(&format!(
                "Couldn't parse file: path={} error={}",
                path, e
            )),
            &"A TOML-formatted file",
        )
    })?;

    let res = clients
        .get("client")
        .and_then(Item::as_array_of_tables)
        .ok_or(serde::de::Error::invalid_value(
            serde::de::Unexpected::Other("Not an array of clients"),
            &"An array of clients",
        ))?
        .iter()
        .map(|x| Client {
            username: demangle_toml_string(x["username"].to_string()),
            password: demangle_toml_string(x["password"].to_string()),
        })
        .collect();

    Ok(res)
}

fn deserialize_rules<'de, D>(deserializer: D) -> Result<Option<rules::RulesEngine>, D::Error>
where
    D: serde::de::Deserializer<'de>,
{
    let path = match deserialize_file_path(deserializer) {
        Ok(path) => path,
        Err(_) => {
            // No rules file specified, default to allow all
            return Ok(Some(rules::RulesEngine::default_allow()));
        }
    };

    let content = match std::fs::read_to_string(&path) {
        Ok(content) => content,
        Err(e) => {
            // Log warning but don't fail - default to allow all
            eprintln!(
                "Warning: Could not read rules file '{}': {}. Defaulting to allow all connections.",
                path, e
            );
            return Ok(Some(rules::RulesEngine::default_allow()));
        }
    };

    let rules_doc: Document = match content.parse() {
        Ok(doc) => doc,
        Err(e) => {
            eprintln!("Warning: Could not parse rules file '{}': {}. Defaulting to allow all connections.", path, e);
            return Ok(Some(rules::RulesEngine::default_allow()));
        }
    };

    let rules_config = match rules_doc.get("rule").and_then(Item::as_array_of_tables) {
        Some(rules_array) => {
            let rules: Vec<rules::Rule> = rules_array
                .iter()
                .filter_map(|rule_table| {
                    let cidr = rule_table
                        .get("cidr")
                        .and_then(Item::as_str)
                        .map(|s| s.to_string());

                    let client_random_prefix = rule_table
                        .get("client_random_prefix")
                        .and_then(Item::as_str)
                        .map(|s| s.to_string());

                    let action = rule_table
                        .get("action")
                        .and_then(Item::as_str)
                        .and_then(|s| match s {
                            "allow" => Some(rules::RuleAction::Allow),
                            "deny" => Some(rules::RuleAction::Deny),
                            _ => None,
                        })?;

                    Some(rules::Rule {
                        cidr,
                        client_random_prefix,
                        action,
                    })
                })
                .collect();

            rules::RulesConfig { rule: rules }
        }
        None => {
            // No rules array found, create empty config
            rules::RulesConfig { rule: vec![] }
        }
    };

    Ok(Some(rules::RulesEngine::from_config(rules_config)))
}

fn demangle_toml_string(x: String) -> String {
    x.replace('"', "").trim().to_string()
}
