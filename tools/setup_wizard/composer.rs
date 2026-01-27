use crate::template_settings;
use std::iter::once;
use toml_edit::{value, Document};
use trusttunnel::settings::{
    ForwardProtocolSettings, Http1Settings, Http2Settings, IcmpSettings, ListenProtocolSettings,
    MetricsSettings, QuicSettings, Settings,
};
use trusttunnel::utils::{IterJoin, ToTomlComment};

pub fn compose_document(settings: &Settings, credentials_path: &str, rules_path: &str) -> String {
    once(compose_main_table(settings, credentials_path, rules_path))
        .chain(once(compose_forward_protocol_table(
            settings.get_forward_protocol(),
        )))
        .chain(once(compose_listener_protocol_table(
            settings.get_listen_protocols(),
        )))
        .chain(once(compose_icmp_table(settings.get_icmp().as_ref())))
        .chain(once(compose_metrics_table(settings.get_metrics().as_ref())))
        .join("\n")
}

fn compose_main_table(settings: &Settings, credentials_path: &str, rules_path: &str) -> String {
    let mut doc: Document = template_settings::MAIN_TABLE.parse().unwrap();

    doc["listen_address"] = value(settings.get_listen_address().to_string());
    doc["credentials_file"] = value(credentials_path);
    doc["rules_file"] = value(rules_path);
    doc["ipv6_available"] = value(*settings.get_ipv6_available());
    doc["allow_private_network_connections"] =
        value(*settings.get_allow_private_network_connections());
    doc["tls_handshake_timeout_secs"] =
        value(settings.get_tls_handshake_timeout().as_secs() as i64);
    doc["client_listener_timeout_secs"] =
        value(settings.get_client_listener_timeout().as_secs() as i64);
    doc["connection_establishment_timeout_secs"] =
        value(settings.get_connection_establishment_timeout().as_secs() as i64);
    doc["tcp_connections_timeout_secs"] =
        value(settings.get_tcp_connections_timeout().as_secs() as i64);
    doc["udp_connections_timeout_secs"] =
        value(settings.get_udp_connections_timeout().as_secs() as i64);
    doc["speedtest_enable"] = value(*settings.get_speedtest_enable());
    doc["ping_enable"] = value(*settings.get_ping_enable());
    doc["allow_without_token"] = value(*settings.get_allow_without_token());

    if let Some(path) = settings.get_ping_path().as_ref() {
        doc["ping_path"] = value(path.clone());
    } else {
        doc.remove("ping_path");
    }
    if let Some(path) = settings.get_speedtest_path().as_ref() {
        doc["speedtest_path"] = value(path.clone());
    } else {
        doc.remove("speedtest_path");
    }
    doc.remove("tunnel_token");

    doc.to_string()
}

fn compose_forward_protocol_table(settings: &ForwardProtocolSettings) -> String {
    let spec = match settings {
        ForwardProtocolSettings::Direct(_) => template_settings::DIRECT_FORWARDER_TABLE.clone(),
        ForwardProtocolSettings::Socks5(x) => {
            let mut doc: Document = template_settings::SOCKS_FORWARDER_TABLE.parse().unwrap();
            let table = doc["forward_protocol"]["socks5"].as_table_mut().unwrap();
            table["address"] = value(x.get_address().to_string());
            table["extended_auth"] = value(*x.get_extended_auth());
            doc.to_string()
        }
    };

    format!(
        "{}\n{}",
        *template_settings::FORWARD_PROTOCOL_COMMON_TABLE,
        spec
    )
}

fn compose_listener_protocol_table(settings: &ListenProtocolSettings) -> String {
    once(template_settings::LISTENER_COMMON_TABLE.clone())
        .chain(once(compose_http1_listener_table(settings.http1.as_ref())))
        .chain(once(compose_http2_listener_table(settings.http2.as_ref())))
        .chain(once(compose_quic_listener_table(settings.quic.as_ref())))
        .join("\n")
}

fn compose_http1_listener_table(settings: Option<&Http1Settings>) -> String {
    match settings {
        Some(x) => {
            let mut doc: Document = template_settings::HTTP1_LISTENER_TABLE.parse().unwrap();
            let table = doc["listen_protocols"]["http1"].as_table_mut().unwrap();

            table["upload_buffer_size"] = value(*x.get_upload_buffer_size() as i64);

            doc.to_string()
        }
        None => template_settings::HTTP1_LISTENER_TABLE.to_toml_comment(),
    }
}

fn compose_http2_listener_table(settings: Option<&Http2Settings>) -> String {
    match settings {
        Some(x) => {
            let mut doc: Document = template_settings::HTTP2_LISTENER_TABLE.parse().unwrap();
            let table = doc["listen_protocols"]["http2"].as_table_mut().unwrap();

            table["initial_connection_window_size"] =
                value(*x.get_initial_connection_window_size() as i64);
            table["initial_stream_window_size"] = value(*x.get_initial_stream_window_size() as i64);
            table["max_concurrent_streams"] = value(*x.get_max_concurrent_streams() as i64);
            table["max_frame_size"] = value(*x.get_max_frame_size() as i64);
            table["header_table_size"] = value(*x.get_header_table_size() as i64);

            doc.to_string()
        }
        None => template_settings::HTTP2_LISTENER_TABLE.to_toml_comment(),
    }
}

fn compose_quic_listener_table(settings: Option<&QuicSettings>) -> String {
    match settings {
        Some(x) => {
            let mut doc: Document = template_settings::QUIC_LISTENER_TABLE.parse().unwrap();
            let table = doc["listen_protocols"]["quic"].as_table_mut().unwrap();

            table["recv_udp_payload_size"] = value(*x.get_recv_udp_payload_size() as i64);
            table["send_udp_payload_size"] = value(*x.get_send_udp_payload_size() as i64);
            table["initial_max_data"] = value(*x.get_initial_max_data() as i64);
            table["max_stream_data_bidi_local"] =
                value(*x.get_initial_max_stream_data_bidi_local() as i64);
            table["max_stream_data_bidi_remote"] =
                value(*x.get_initial_max_stream_data_bidi_remote() as i64);
            table["max_stream_data_uni"] = value(*x.get_initial_max_stream_data_uni() as i64);
            table["max_streams_bidi"] = value(*x.get_initial_max_streams_bidi() as i64);
            table["max_streams_uni"] = value(*x.get_initial_max_streams_uni() as i64);
            table["max_connection_window"] = value(*x.get_max_connection_window() as i64);
            table["max_stream_window"] = value(*x.get_max_stream_window() as i64);
            table["disable_active_migration"] = value(*x.get_disable_active_migration());
            table["enable_early_data"] = value(*x.get_enable_early_data());
            table["message_queue_capacity"] = value(*x.get_message_queue_capacity() as i64);

            doc.to_string()
        }
        None => template_settings::QUIC_LISTENER_TABLE.to_toml_comment(),
    }
}

fn compose_icmp_table(settings: Option<&IcmpSettings>) -> String {
    match settings {
        Some(x) => {
            let mut doc: Document = template_settings::ICMP_TABLE.parse().unwrap();
            let table = doc["icmp"].as_table_mut().unwrap();

            table["interface_name"] = value(x.get_interface_name());
            table["request_timeout_secs"] = value(x.get_request_timeout().as_secs() as i64);
            table["recv_message_queue_capacity"] =
                value(*x.get_recv_message_queue_capacity() as i64);

            doc.to_string()
        }
        None => template_settings::ICMP_TABLE.to_toml_comment(),
    }
}

fn compose_metrics_table(settings: Option<&MetricsSettings>) -> String {
    match settings {
        Some(x) => {
            let mut doc: Document = template_settings::METRICS_TABLE.parse().unwrap();
            let table = doc["metrics"].as_table_mut().unwrap();

            table["address"] = value(x.get_address().to_string());
            table["request_timeout_secs"] = value(x.get_request_timeout().as_secs() as i64);

            doc.to_string()
        }
        None => template_settings::METRICS_TABLE.to_toml_comment(),
    }
}
