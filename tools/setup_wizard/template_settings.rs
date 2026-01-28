use once_cell::sync::Lazy;
use trusttunnel::settings::{
    ForwardProtocolSettings, Http1Settings, Http2Settings, IcmpSettings, ListenProtocolSettings,
    MetricsSettings, QuicSettings, Settings, Socks5ForwarderSettings,
};
use trusttunnel::utils::ToTomlComment;

pub static MAIN_TABLE: Lazy<String> = Lazy::new(|| {
    format!(
        r#"{}
listen_address = ""

# The path to a TOML file in the following format:
#
# ```
# [[client]]
# username = "a"
# password = "b"
#
# [[client]]
# ...
# ```
credentials_file = "{}"

# The path to a TOML file for connection filtering rules in the following format:
#
# ```
# [[rule]]
# cidr = "192.168.0.0/16"
# action = "allow"
#
# [[rule]]
# client_random_prefix = "aabbcc"
# action = "deny"
#
# [[rule]]
# client_random_prefix = "a0b0/f0f0"  # Format: prefix[/mask] for bitwise matching
# action = "allow"
#
# [[rule]]
# action = "deny"
#
# If no rules in this file, all connections are allowed by default.
# ```
rules_file = "{}"

{}
ipv6_available = {}

{}
allow_private_network_connections = {}

{}
tls_handshake_timeout_secs = {}

{}
client_listener_timeout_secs = {}

{}
connection_establishment_timeout_secs = {}

{}
tcp_connections_timeout_secs = {}

{}
udp_connections_timeout_secs = {}

{}
speedtest_enable = {}
"#,
        Settings::doc_listen_address().to_toml_comment(),
        crate::library_settings::DEFAULT_CREDENTIALS_PATH,
        crate::library_settings::DEFAULT_RULES_PATH,
        Settings::doc_ipv6_available().to_toml_comment(),
        Settings::default_ipv6_available(),
        Settings::doc_allow_private_network_connections().to_toml_comment(),
        Settings::default_allow_private_network_connections(),
        format!("{}. In seconds.", Settings::doc_tls_handshake_timeout()).to_toml_comment(),
        Settings::default_tls_handshake_timeout().as_secs(),
        format!("{}. In seconds.", Settings::doc_client_listener_timeout()).to_toml_comment(),
        Settings::default_client_listener_timeout().as_secs(),
        format!(
            "{} In seconds.",
            Settings::doc_connection_establishment_timeout()
        )
        .to_toml_comment(),
        Settings::default_connection_establishment_timeout().as_secs(),
        format!("{}. In seconds.", Settings::doc_tcp_connections_timeout()).to_toml_comment(),
        Settings::default_tcp_connections_timeout().as_secs(),
        format!("{}. In seconds.", Settings::doc_udp_connections_timeout()).to_toml_comment(),
        Settings::default_udp_connections_timeout().as_secs(),
        Settings::doc_speedtest_enable().to_toml_comment(),
        Settings::default_speedtest_enable(),
    )
});

pub static FORWARD_PROTOCOL_COMMON_TABLE: Lazy<String> = Lazy::new(|| {
    format!(
        r#"{}.
# Possible values:
#   * direct: a direct forwarder routes a connection directly to its target host,
#   * socks5: a SOCKS5 forwarder routes a connection though a SOCKS5 proxy.
# Default is direct
[forward_protocol]
"#,
        ForwardProtocolSettings::doc().to_toml_comment(),
    )
});

pub static DIRECT_FORWARDER_TABLE: Lazy<String> = Lazy::new(|| {
    format!(
        r#"{}.
[forward_protocol.direct]"#,
        ForwardProtocolSettings::doc_direct().to_toml_comment(),
    )
});

pub static SOCKS_FORWARDER_TABLE: Lazy<String> = Lazy::new(|| {
    format!(
        r#"{}.
[forward_protocol.socks5]
{}
address = "127.0.0.1:1080"
{}
extended_auth = false"#,
        ForwardProtocolSettings::doc_socks5().to_toml_comment(),
        Socks5ForwarderSettings::doc_address().to_toml_comment(),
        Socks5ForwarderSettings::doc_extended_auth().to_toml_comment(),
    )
});

pub static LISTENER_COMMON_TABLE: Lazy<String> = Lazy::new(|| {
    format!(
        r#"{}.
# Possible values:
#   * http1: enables HTTP1 codec,
#   * http2: enables HTTP2 codec,
#   * quic: enables QUIC/HTTP3 codec.
# At least one listener codec MUST be specified.
[listen_protocols]
"#,
        ListenProtocolSettings::doc().to_toml_comment(),
    )
});

pub static HTTP1_LISTENER_TABLE: Lazy<String> = Lazy::new(|| {
    format!(
        r#"{}.
[listen_protocols.http1]
{}
upload_buffer_size = {}
"#,
        Http1Settings::doc().to_toml_comment(),
        Http1Settings::doc_upload_buffer_size().to_toml_comment(),
        Http1Settings::default_upload_buffer_size(),
    )
});

pub static HTTP2_LISTENER_TABLE: Lazy<String> = Lazy::new(|| {
    format!(
        r#"{}.
[listen_protocols.http2]
{}
initial_connection_window_size = {}
{}
initial_stream_window_size = {}
{}
max_concurrent_streams = {}
{}
max_frame_size = {}
{}
header_table_size = {}
"#,
        Http2Settings::doc().to_toml_comment(),
        Http2Settings::doc_initial_connection_window_size().to_toml_comment(),
        Http2Settings::default_initial_connection_window_size(),
        Http2Settings::doc_initial_stream_window_size().to_toml_comment(),
        Http2Settings::default_initial_stream_window_size(),
        Http2Settings::doc_max_concurrent_streams().to_toml_comment(),
        Http2Settings::default_max_concurrent_streams(),
        Http2Settings::doc_max_frame_size().to_toml_comment(),
        Http2Settings::default_max_frame_size(),
        Http2Settings::doc_header_table_size().to_toml_comment(),
        Http2Settings::default_header_table_size(),
    )
});

pub static QUIC_LISTENER_TABLE: Lazy<String> = Lazy::new(|| {
    format!(
        r#"{}.
[listen_protocols.quic]
{}
recv_udp_payload_size = {}
{}
send_udp_payload_size = {}
{}
initial_max_data = {}
{}
initial_max_stream_data_bidi_local = {}
{}
initial_max_stream_data_bidi_remote = {}
{}
initial_max_stream_data_uni = {}
{}
initial_max_streams_bidi = {}
{}
initial_max_streams_uni = {}
{}
max_connection_window = {}
{}
max_stream_window = {}
{}
disable_active_migration = {}
{}
enable_early_data = {}
{}
message_queue_capacity = {}
"#,
        QuicSettings::doc().to_toml_comment(),
        QuicSettings::doc_recv_udp_payload_size().to_toml_comment(),
        QuicSettings::default_recv_udp_payload_size(),
        QuicSettings::doc_send_udp_payload_size().to_toml_comment(),
        QuicSettings::default_send_udp_payload_size(),
        QuicSettings::doc_initial_max_data().to_toml_comment(),
        QuicSettings::default_initial_max_data(),
        QuicSettings::doc_initial_max_stream_data_bidi_local().to_toml_comment(),
        QuicSettings::default_initial_max_stream_data_bidi_local(),
        QuicSettings::doc_initial_max_stream_data_bidi_remote().to_toml_comment(),
        QuicSettings::default_initial_max_stream_data_bidi_remote(),
        QuicSettings::doc_initial_max_stream_data_uni().to_toml_comment(),
        QuicSettings::default_initial_max_stream_data_uni(),
        QuicSettings::doc_initial_max_streams_bidi().to_toml_comment(),
        QuicSettings::default_initial_max_streams_bidi(),
        QuicSettings::doc_initial_max_streams_uni().to_toml_comment(),
        QuicSettings::default_initial_max_streams_uni(),
        QuicSettings::doc_max_connection_window().to_toml_comment(),
        QuicSettings::default_max_connection_window(),
        QuicSettings::doc_max_stream_window().to_toml_comment(),
        QuicSettings::default_max_stream_window(),
        QuicSettings::doc_disable_active_migration().to_toml_comment(),
        QuicSettings::default_disable_active_migration(),
        QuicSettings::doc_enable_early_data().to_toml_comment(),
        QuicSettings::default_enable_early_data(),
        QuicSettings::doc_message_queue_capacity().to_toml_comment(),
        QuicSettings::default_message_queue_capacity(),
    )
});

pub static ICMP_TABLE: Lazy<String> = Lazy::new(|| {
    format!(
        r#"{}
[icmp]
{}
interface_name = "{}"
{}
request_timeout_secs = {}
{}
recv_message_queue_capacity = {}
"#,
        IcmpSettings::doc().to_toml_comment(),
        IcmpSettings::doc_interface_name().to_toml_comment(),
        IcmpSettings::default_interface_name(),
        IcmpSettings::doc_request_timeout().to_toml_comment(),
        IcmpSettings::default_request_timeout().as_secs(),
        IcmpSettings::doc_recv_message_queue_capacity().to_toml_comment(),
        IcmpSettings::default_message_queue_capacity(),
    )
});

pub static METRICS_TABLE: Lazy<String> = Lazy::new(|| {
    format!(
        r#"{}
[metrics]
{}
address = "{}"
{}
request_timeout_secs = {}
"#,
        MetricsSettings::doc().to_toml_comment(),
        MetricsSettings::doc_address().to_toml_comment(),
        MetricsSettings::default_listen_address(),
        MetricsSettings::doc_request_timeout().to_toml_comment(),
        MetricsSettings::default_request_timeout().as_secs(),
    )
});
