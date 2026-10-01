use futures::future;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use trusttunnel::authentication::registry_based::Client;
use trusttunnel::authentication::{registry_based::RegistryBasedAuthenticator, Authenticator};
use trusttunnel::core::Core;
use trusttunnel::net_utils;
use trusttunnel::settings::{
    Http1Settings, Http2Settings, ListenProtocolSettings, QuicSettings, Settings, TlsHostInfo,
    TlsHostsSettings,
};
use trusttunnel::shutdown::Shutdown;
use trusttunnel::subscription::SubscriptionSettings;

#[allow(dead_code)]
mod common;

fn make_settings(
    listen_address: &SocketAddr,
    subscription_address: &str,
    enabled: bool,
    users: Vec<Client>,
) -> Settings {
    let mut builder = Settings::builder()
        .listen_address(listen_address)
        .unwrap()
        .listen_protocols(ListenProtocolSettings {
            http1: Some(Http1Settings::builder().build()),
            http2: Some(Http2Settings::builder().build()),
            quic: Some(QuicSettings::builder().build()),
        })
        .allow_private_network_connections(true)
        .clients(users);

    if enabled {
        builder = builder.subscription(SubscriptionSettings {
            enabled: true,
            hostname: Some(common::MAIN_DOMAIN_NAME.to_string()),
            path: SubscriptionSettings::default_path(),
            address: Some(subscription_address.to_string()),
            name: Some("Acme Corp VPN".to_string()),
            dns_upstreams: vec!["tls://1.1.1.1".to_string()],
            custom_sni: Some(common::MAIN_DOMAIN_NAME.to_string()),
            client_random_prefix: Some("aabbcc/ff00ff".to_string()),
        });
    }

    builder.build().unwrap()
}

fn make_hosts(cert_path: &str) -> TlsHostsSettings {
    make_hosts_with_names(cert_path, &[common::MAIN_DOMAIN_NAME])
}

fn make_hosts_with_names(cert_path: &str, names: &[&str]) -> TlsHostsSettings {
    let main_hosts: Vec<TlsHostInfo> = names
        .iter()
        .map(|&name| TlsHostInfo {
            hostname: name.to_string(),
            cert_chain_path: cert_path.to_string(),
            private_key_path: cert_path.to_string(),
            allowed_sni: vec![],
        })
        .collect();
    TlsHostsSettings::builder()
        .main_hosts(main_hosts)
        .build()
        .unwrap()
}

/// Like [`make_settings`] with an enabled subscription, but with the optional
/// `custom_sni` / `client_random_prefix` / `name` / `dns_upstreams` fields
/// unset, so omission in the JSON response can be asserted.
fn make_minimal_subscription_settings(
    listen_address: &SocketAddr,
    subscription_address: &str,
    users: Vec<Client>,
) -> Settings {
    Settings::builder()
        .listen_address(listen_address)
        .unwrap()
        .listen_protocols(ListenProtocolSettings {
            http1: Some(Http1Settings::builder().build()),
            http2: Some(Http2Settings::builder().build()),
            quic: Some(QuicSettings::builder().build()),
        })
        .allow_private_network_connections(true)
        .clients(users)
        .subscription(SubscriptionSettings {
            enabled: true,
            hostname: Some(common::MAIN_DOMAIN_NAME.to_string()),
            path: SubscriptionSettings::default_path(),
            address: Some(subscription_address.to_string()),
            name: None,
            dns_upstreams: vec![],
            custom_sni: None,
            client_random_prefix: None,
        })
        .build()
        .unwrap()
}

fn alice_bob() -> Vec<Client> {
    vec![
        Client {
            username: "alice".to_string(),
            password: "secret".to_string(),
            max_http2_conns: None,
            max_http3_conns: None,
        },
        Client {
            username: "bob".to_string(),
            password: "passwd".to_string(),
            max_http2_conns: None,
            max_http3_conns: None,
        },
    ]
}

// base64("alice:secret") and base64("bob:passwd")
const ALICE_BASIC: &str = "Basic YWxpY2U6c2VjcmV0";
const BOB_BASIC: &str = "Basic Ym9iOnBhc3N3ZA==";

async fn fetch(
    endpoint_address: &SocketAddr,
    auth: Option<&str>,
    extra: &[(&str, &str)],
) -> (http::StatusCode, bytes::Bytes) {
    let stream =
        common::establish_tls_connection(common::MAIN_DOMAIN_NAME, endpoint_address, None).await;
    let url = format!(
        "https://{}:{}/subscription",
        common::MAIN_DOMAIN_NAME,
        endpoint_address.port()
    );
    let mut headers: Vec<(&str, &str)> = Vec::new();
    if let Some(a) = auth {
        headers.push(("authorization", a));
    }
    headers.extend_from_slice(extra);
    let (parts, body) =
        common::do_get_request(stream, http::Version::HTTP_11, &url, &headers).await;
    (parts.status, body)
}

#[tokio::test]
async fn get_with_valid_credentials_returns_200_and_user_fields() {
    common::set_up_logger();
    let endpoint_address = common::make_endpoint_address();
    let cert_file = common::make_cert_key_file();
    let cert_path = cert_file.path.to_str().unwrap();
    let settings = make_settings(&endpoint_address, "203.0.113.1:443", true, alice_bob());
    let hosts = make_hosts(cert_path);

    let client_task = async {
        tokio::time::sleep(Duration::from_secs(1)).await;
        let (status, body) = fetch(&endpoint_address, Some(ALICE_BASIC), &[]).await;
        assert_eq!(status, http::StatusCode::OK);
        let body = String::from_utf8(body.to_vec()).unwrap();
        assert!(body.contains("\"version\":1"));
        assert!(body.contains("\"hostname\":\"localhost\""));
        assert!(body.contains("\"address\":\"203.0.113.1:443\""));
        assert!(body.contains("\"username\":\"alice\""));
        assert!(body.contains("\"password\":\"secret\""));
        assert!(body.contains("\"name\":\"Acme Corp VPN\""));
        assert!(body.contains("\"dns_upstreams\":[\"tls://1.1.1.1\"]"));
        assert!(body.contains("\"custom_sni\":\"localhost\""));
        assert!(body.contains("\"client_random_prefix\":\"aabbcc/ff00ff\""));
    };

    tokio::select! {
        _ = run_endpoint(settings, hosts) => unreachable!(),
        _ = client_task => (),
        _ = tokio::time::sleep(Duration::from_secs(10)) => panic!("Timed out"),
    }
}

#[tokio::test]
async fn get_without_credentials_returns_401() {
    common::set_up_logger();
    let endpoint_address = common::make_endpoint_address();
    let cert_file = common::make_cert_key_file();
    let settings = make_settings(&endpoint_address, "203.0.113.1:443", true, alice_bob());
    let hosts = make_hosts(cert_file.path.to_str().unwrap());

    let client_task = async {
        tokio::time::sleep(Duration::from_secs(1)).await;
        let (status, body) = fetch(&endpoint_address, None, &[]).await;
        assert_eq!(status, http::StatusCode::UNAUTHORIZED);
        assert!(!body.windows(6).any(|w| w == b"secret"));
    };

    tokio::select! {
        _ = run_endpoint(settings, hosts) => unreachable!(),
        _ = client_task => (),
        _ = tokio::time::sleep(Duration::from_secs(10)) => panic!("Timed out"),
    }
}

#[tokio::test]
async fn get_as_bob_returns_bobs_credentials_not_alices() {
    common::set_up_logger();
    let endpoint_address = common::make_endpoint_address();
    let cert_file = common::make_cert_key_file();
    let settings = make_settings(&endpoint_address, "203.0.113.1:443", true, alice_bob());
    let hosts = make_hosts(cert_file.path.to_str().unwrap());

    let client_task = async {
        tokio::time::sleep(Duration::from_secs(1)).await;
        let (status, body) = fetch(&endpoint_address, Some(BOB_BASIC), &[]).await;
        assert_eq!(status, http::StatusCode::OK);
        let body = String::from_utf8(body.to_vec()).unwrap();
        assert!(body.contains("\"username\":\"bob\""));
        assert!(body.contains("\"password\":\"passwd\""));
        assert!(!body.contains("secret"));
    };

    tokio::select! {
        _ = run_endpoint(settings, hosts) => unreachable!(),
        _ = client_task => (),
        _ = tokio::time::sleep(Duration::from_secs(10)) => panic!("Timed out"),
    }
}

#[tokio::test]
async fn post_without_credentials_returns_401() {
    common::set_up_logger();
    let endpoint_address = common::make_endpoint_address();
    let cert_file = common::make_cert_key_file();
    let settings = make_settings(&endpoint_address, "203.0.113.1:443", true, alice_bob());
    let hosts = make_hosts(cert_file.path.to_str().unwrap());

    let client_task = async {
        tokio::time::sleep(Duration::from_secs(1)).await;
        let stream =
            common::establish_tls_connection(common::MAIN_DOMAIN_NAME, &endpoint_address, None)
                .await;
        let url = format!(
            "https://{}:{}/subscription",
            common::MAIN_DOMAIN_NAME,
            endpoint_address.port()
        );
        let response = common::do_post_request(stream, http::Version::HTTP_11, &url, 0).await;
        // Auth runs before the method check, so an unauthenticated non-GET
        // request gets 401 (not 405) and does not confirm the path.
        assert_eq!(response.status(), http::StatusCode::UNAUTHORIZED);
    };

    tokio::select! {
        _ = run_endpoint(settings, hosts) => unreachable!(),
        _ = client_task => (),
        _ = tokio::time::sleep(Duration::from_secs(10)) => panic!("Timed out"),
    }
}

#[tokio::test]
async fn post_with_credentials_returns_405() {
    common::set_up_logger();
    let endpoint_address = common::make_endpoint_address();
    let cert_file = common::make_cert_key_file();
    let settings = make_settings(&endpoint_address, "203.0.113.1:443", true, alice_bob());
    let hosts = make_hosts(cert_file.path.to_str().unwrap());

    let client_task = async {
        tokio::time::sleep(Duration::from_secs(1)).await;
        let stream =
            common::establish_tls_connection(common::MAIN_DOMAIN_NAME, &endpoint_address, None)
                .await;
        let url = format!(
            "https://{}:{}/subscription",
            common::MAIN_DOMAIN_NAME,
            endpoint_address.port()
        );
        let (mut request, conn) = hyper::client::conn::Builder::new()
            .handshake(stream)
            .await
            .unwrap();
        let exchange = async {
            let req = hyper::Request::post(&url)
                .version(http::Version::HTTP_11)
                .header("authorization", ALICE_BASIC)
                .body(hyper::Body::empty())
                .unwrap();
            request.send_request(req).await.unwrap()
        };
        futures::pin_mut!(exchange);
        let response = match future::select(conn, exchange).await {
            future::Either::Left((_r, exchange)) => exchange.await,
            future::Either::Right((response, _)) => response,
        };
        assert_eq!(response.status(), http::StatusCode::METHOD_NOT_ALLOWED);
    };

    tokio::select! {
        _ = run_endpoint(settings, hosts) => unreachable!(),
        _ = client_task => (),
        _ = tokio::time::sleep(Duration::from_secs(10)) => panic!("Timed out"),
    }
}

#[tokio::test]
async fn disabled_subscription_does_not_serve_data() {
    common::set_up_logger();
    let endpoint_address = common::make_endpoint_address();
    let cert_file = common::make_cert_key_file();
    // Absent section (make_settings with enabled=false adds no [subscription]);
    // the path is not reserved, so the request falls through to the tunnel.
    let settings = make_settings(&endpoint_address, "203.0.113.1:443", false, alice_bob());
    let hosts = make_hosts(cert_file.path.to_str().unwrap());

    let client_task = async {
        tokio::time::sleep(Duration::from_secs(1)).await;
        let (status, body) = fetch(&endpoint_address, Some(ALICE_BASIC), &[]).await;
        assert_ne!(status, http::StatusCode::OK);
        assert!(!body.windows(6).any(|w| w == b"secret"));
    };

    tokio::select! {
        _ = run_endpoint(settings, hosts) => unreachable!(),
        _ = client_task => (),
        _ = tokio::time::sleep(Duration::from_secs(10)) => panic!("Timed out"),
    }
}

#[tokio::test]
async fn present_disabled_section_returns_403() {
    common::set_up_logger();
    let endpoint_address = common::make_endpoint_address();
    let cert_file = common::make_cert_key_file();
    let cert_path = cert_file.path.to_str().unwrap();
    // Section present but disabled: the path is reserved and the handler
    // returns 403 instead of falling through to the tunnel.
    let settings = Settings::builder()
        .listen_address(&endpoint_address)
        .unwrap()
        .listen_protocols(ListenProtocolSettings {
            http1: Some(Http1Settings::builder().build()),
            http2: Some(Http2Settings::builder().build()),
            quic: Some(QuicSettings::builder().build()),
        })
        .allow_private_network_connections(true)
        .clients(alice_bob())
        // Fully specified even though disabled: `validate` checks `address`
        // unconditionally now, so a staged disabled section must be valid to load.
        .subscription(SubscriptionSettings {
            enabled: false,
            hostname: Some(common::MAIN_DOMAIN_NAME.to_string()),
            path: SubscriptionSettings::default_path(),
            address: Some("203.0.113.1:443".to_string()),
            name: None,
            dns_upstreams: vec![],
            custom_sni: None,
            client_random_prefix: None,
        })
        .build()
        .unwrap();
    let hosts = make_hosts(cert_path);

    let client_task = async {
        tokio::time::sleep(Duration::from_secs(1)).await;
        let (status, body) = fetch(&endpoint_address, Some(ALICE_BASIC), &[]).await;
        assert_eq!(status, http::StatusCode::FORBIDDEN);
        assert!(!body.windows(6).any(|w| w == b"secret"));
    };

    tokio::select! {
        _ = run_endpoint(settings, hosts) => unreachable!(),
        _ = client_task => (),
        _ = tokio::time::sleep(Duration::from_secs(10)) => panic!("Timed out"),
    }
}

async fn run_endpoint(settings: Settings, hosts: TlsHostsSettings) {
    let shutdown = Shutdown::new();
    let authenticator: Option<Arc<dyn Authenticator>> = if !settings.get_clients().is_empty() {
        Some(Arc::new(RegistryBasedAuthenticator::new(
            settings.get_clients(),
        )))
    } else {
        None
    };
    let endpoint = Core::new(settings, authenticator, hosts, shutdown).unwrap();
    endpoint.listen().await.unwrap();
}

async fn fetch_address(endpoint_address: &SocketAddr, auth: &str) -> String {
    let stream =
        common::establish_tls_connection(common::MAIN_DOMAIN_NAME, endpoint_address, None).await;
    let url = format!(
        "https://{}:{}/subscription",
        common::MAIN_DOMAIN_NAME,
        endpoint_address.port()
    );
    let (parts, body) = common::do_get_request(
        stream,
        http::Version::HTTP_11,
        &url,
        &[("authorization", auth)],
    )
    .await;
    assert_eq!(parts.status, http::StatusCode::OK);
    let body = String::from_utf8(body.to_vec()).unwrap();
    body.split("\"address\":\"")
        .nth(1)
        .unwrap()
        .split('"')
        .next()
        .unwrap()
        .to_string()
}

#[tokio::test]
async fn get_h2_authenticated_returns_json() {
    common::set_up_logger();
    let endpoint_address = common::make_endpoint_address();
    let cert_file = common::make_cert_key_file();
    let cert_path = cert_file.path.to_str().unwrap();
    let settings = make_settings(&endpoint_address, "203.0.113.1:443", true, alice_bob());
    let hosts = make_hosts(cert_path);

    let client_task = async {
        tokio::time::sleep(Duration::from_secs(1)).await;
        let stream = common::establish_tls_connection(
            common::MAIN_DOMAIN_NAME,
            &endpoint_address,
            Some(net_utils::HTTP2_ALPN.as_bytes()),
        )
        .await;
        let url = format!(
            "https://{}:{}/subscription",
            common::MAIN_DOMAIN_NAME,
            endpoint_address.port()
        );
        // Verify the HTTP/2 path routes to the subscription handler and
        // authenticates. The test harness cannot read a streamed H2 response
        // body (the connection driver is an independent task), so body content
        // is asserted over HTTP/1.1 in get_with_valid_credentials_*.
        let (mut request_sender, conn) = hyper::client::conn::Builder::new()
            .http2_only(true)
            .handshake(stream)
            .await
            .unwrap();
        let exchange = async {
            let req = hyper::Request::builder()
                .method("GET")
                .uri(&url)
                .version(http::Version::HTTP_2)
                .header("authorization", ALICE_BASIC)
                .body(hyper::Body::empty())
                .unwrap();
            request_sender.send_request(req).await.unwrap()
        };
        futures::pin_mut!(exchange);
        let response = match future::select(conn, exchange).await {
            future::Either::Left((_r, exchange)) => exchange.await,
            future::Either::Right((response, _)) => response,
        };
        assert_eq!(response.status(), http::StatusCode::OK);
    };

    tokio::select! {
        _ = run_endpoint(settings, hosts) => unreachable!(),
        _ = client_task => (),
        _ = tokio::time::sleep(Duration::from_secs(10)) => panic!("Timed out"),
    }
}

#[tokio::test]
async fn omits_optional_fields_when_unset() {
    common::set_up_logger();
    let endpoint_address = common::make_endpoint_address();
    let cert_file = common::make_cert_key_file();
    let cert_path = cert_file.path.to_str().unwrap();
    let settings =
        make_minimal_subscription_settings(&endpoint_address, "203.0.113.1:443", alice_bob());
    let hosts = make_hosts(cert_path);

    let client_task = async {
        tokio::time::sleep(Duration::from_secs(1)).await;
        let (status, body) = fetch(&endpoint_address, Some(ALICE_BASIC), &[]).await;
        assert_eq!(status, http::StatusCode::OK);
        let body = String::from_utf8(body.to_vec()).unwrap();
        assert!(body.contains("\"version\":1"));
        assert!(body.contains("\"has_ipv6\":true"));
        assert!(!body.contains("custom_sni"));
        assert!(!body.contains("client_random_prefix"));
        assert!(!body.contains("\"name\""));
        assert!(!body.contains("dns_upstreams"));
    };

    tokio::select! {
        _ = run_endpoint(settings, hosts) => unreachable!(),
        _ = client_task => (),
        _ = tokio::time::sleep(Duration::from_secs(10)) => panic!("Timed out"),
    }
}

#[tokio::test]
async fn get_sni_mismatch_returns_404() {
    common::set_up_logger();
    let endpoint_address = common::make_endpoint_address();
    let cert_file = common::make_cert_key_file();
    let cert_path = cert_file.path.to_str().unwrap();
    // Subscription hostname is MAIN_DOMAIN_NAME; serve a second main host so a
    // connection with a different SNI is still accepted by TLS.
    let settings = make_settings(&endpoint_address, "203.0.113.1:443", true, alice_bob());
    let hosts = make_hosts_with_names(cert_path, &[common::MAIN_DOMAIN_NAME, "other.test"]);

    let client_task = async {
        tokio::time::sleep(Duration::from_secs(1)).await;
        let stream = common::establish_tls_connection("other.test", &endpoint_address, None).await;
        let url = format!(
            "https://other.test:{}/subscription",
            endpoint_address.port()
        );
        let (parts, body) = common::do_get_request(
            stream,
            http::Version::HTTP_11,
            &url,
            &[("authorization", ALICE_BASIC)],
        )
        .await;
        assert_eq!(parts.status, http::StatusCode::NOT_FOUND);
        assert!(!body.windows(6).any(|w| w == b"secret"));
    };

    tokio::select! {
        _ = run_endpoint(settings, hosts) => unreachable!(),
        _ = client_task => (),
        _ = tokio::time::sleep(Duration::from_secs(10)) => panic!("Timed out"),
    }
}

#[tokio::test]
async fn reload_updates_served_address_and_keeps_old_on_error() {
    common::set_up_logger();
    let endpoint_address = common::make_endpoint_address();
    let cert_file = common::make_cert_key_file();
    let cert_path = cert_file.path.to_str().unwrap();
    let settings = make_settings(&endpoint_address, "203.0.113.1:443", true, alice_bob());
    let hosts = make_hosts(cert_path);
    let hosts_for_reload = make_hosts(cert_path);

    let shutdown = Shutdown::new();
    let authenticator: Option<Arc<dyn Authenticator>> = Some(Arc::new(
        RegistryBasedAuthenticator::new(settings.get_clients()),
    ));
    let core = Arc::new(Core::new(settings, authenticator, hosts, shutdown).unwrap());

    let core_for_listen = core.clone();
    let listen_task = tokio::spawn(async move {
        core_for_listen.listen().await.unwrap();
    });

    tokio::time::sleep(Duration::from_secs(1)).await;
    assert_eq!(
        fetch_address(&endpoint_address, ALICE_BASIC).await,
        "203.0.113.1:443"
    );

    let updated = SubscriptionSettings {
        enabled: true,
        hostname: Some(common::MAIN_DOMAIN_NAME.to_string()),
        path: SubscriptionSettings::default_path(),
        address: Some("198.51.100.7:443".to_string()),
        name: None,
        dns_upstreams: vec![],
        custom_sni: None,
        client_random_prefix: None,
    };
    core.reload_subscription_settings(Some(updated), &hosts_for_reload)
        .unwrap();
    assert_eq!(
        fetch_address(&endpoint_address, ALICE_BASIC).await,
        "198.51.100.7:443"
    );

    let bad = SubscriptionSettings {
        enabled: true,
        hostname: Some("nope.invalid".to_string()),
        path: SubscriptionSettings::default_path(),
        address: Some("0.0.0.0:0".to_string()),
        name: None,
        dns_upstreams: vec![],
        custom_sni: None,
        client_random_prefix: None,
    };
    assert!(core
        .reload_subscription_settings(Some(bad), &hosts_for_reload)
        .is_err());
    assert_eq!(
        fetch_address(&endpoint_address, ALICE_BASIC).await,
        "198.51.100.7:443"
    );

    listen_task.abort();
}
