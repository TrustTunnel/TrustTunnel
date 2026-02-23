use base64::engine::general_purpose::STANDARD as BASE64_ENGINE;
use base64::Engine;
use futures::future;
use http::Request;
use log::info;
use std::future::Future;
use std::net::{Ipv4Addr, SocketAddr};
use std::time::Duration;
use tokio::io::AsyncReadExt;
use tokio::net::TcpListener;
use trusttunnel::authentication;
use trusttunnel::authentication::jwt::{JwtAlgorithm, JwtAuth, JwtAuthConfig};
use trusttunnel::authentication::AuthProvider;
use trusttunnel::settings::{
    ForwardProtocolSettings, Http1Settings, ListenProtocolSettings, Settings,
    Socks5ForwarderSettings, TlsHostInfo, TlsHostsSettings,
};

#[allow(dead_code)]
mod common;

#[tokio::test]
async fn registry_proxy_auth_success() {
    common::set_up_logger();
    let endpoint_address = common::make_endpoint_address();

    let client_task = async {
        tokio::time::sleep(Duration::from_secs(1)).await;
        let status = do_connect_request(&endpoint_address, Some("a:b".into())).await;
        assert_ne!(status, http::StatusCode::PROXY_AUTHENTICATION_REQUIRED);
    };

    tokio::select! {
        _ = run_endpoint(&endpoint_address, true, None) => unreachable!(),
        _ = client_task => (),
        _ = tokio::time::sleep(Duration::from_secs(10)) => panic!("Timed out"),
    }
}

#[tokio::test]
async fn registry_proxy_auth_failure() {
    common::set_up_logger();
    let endpoint_address = common::make_endpoint_address();

    let client_task = async {
        tokio::time::sleep(Duration::from_secs(1)).await;
        let status = do_connect_request(&endpoint_address, None).await;
        assert_eq!(status, http::StatusCode::PROXY_AUTHENTICATION_REQUIRED);
    };

    tokio::select! {
        _ = run_endpoint(&endpoint_address, true, None) => unreachable!(),
        _ = client_task => (),
        _ = tokio::time::sleep(Duration::from_secs(10)) => panic!("Timed out"),
    }
}

#[tokio::test]
async fn no_authenticator_socks_standard_auth() {
    common::set_up_logger();
    let endpoint_address = common::make_endpoint_address();

    let (socks_addr, socks_task) = make_socks_server_harness();

    let client_task = async {
        tokio::time::sleep(Duration::from_secs(1)).await;
        let _ = do_connect_request(&endpoint_address, Some("a:b".into())).await;
    };

    tokio::select! {
        _ = run_endpoint(&endpoint_address, true, Some(socks_addr)) => unreachable!(),
        _ = client_task => unreachable!(),
        x = socks_task => assert!(x.contains(&0x02), "{:?}", x),
        _ = tokio::time::sleep(Duration::from_secs(10)) => panic!("Timed out"),
    }
}

#[tokio::test]
async fn no_authenticator_no_socks_auth() {
    common::set_up_logger();
    let endpoint_address = common::make_endpoint_address();

    let (socks_addr, socks_task) = make_socks_server_harness();

    let client_task = async {
        tokio::time::sleep(Duration::from_secs(1)).await;
        let _ = do_connect_request(&endpoint_address, None).await;
    };

    tokio::select! {
        _ = run_endpoint(&endpoint_address, false, Some(socks_addr)) => unreachable!(),
        _ = client_task => unreachable!(),
        x = socks_task => assert!(!x.iter().any(|x| *x != 0x00), "Must not contain non-NoAuth methods: {:?}", x),
        _ = tokio::time::sleep(Duration::from_secs(10)) => panic!("Timed out"),
    }
}

#[tokio::test]
async fn authenticator_present_socks_standard_auth() {
    common::set_up_logger();
    let endpoint_address = common::make_endpoint_address();

    let (socks_addr, socks_task) = make_socks_server_harness();

    let client_task = async {
        tokio::time::sleep(Duration::from_secs(1)).await;
        let _ = do_connect_request(&endpoint_address, Some("a:b".into())).await;
    };

    tokio::select! {
        _ = run_endpoint(&endpoint_address, true, Some(socks_addr)) => unreachable!(),
        _ = client_task => unreachable!(),
        x = socks_task => assert!(x.contains(&0x02), "{:?}", x),
        _ = tokio::time::sleep(Duration::from_secs(10)) => panic!("Timed out"),
    }
}

async fn run_endpoint(
    listen_address: &SocketAddr,
    with_auth: bool,
    socks_proxy: Option<SocketAddr>,
) {
    let mut builder = Settings::builder()
        .listen_address(listen_address)
        .unwrap()
        .listen_protocols(ListenProtocolSettings {
            http1: Some(Http1Settings::builder().build()),
            ..Default::default()
        })
        .allow_private_network_connections(true);

    if with_auth {
        builder = builder.clients(Vec::from_iter(std::iter::once(
            authentication::registry_based::Client {
                username: "a".into(),
                password: "b".into(),
                device_user: None,
                device_id: None,
            },
        )));
    }

    if let Some(address) = socks_proxy {
        builder = builder.forwarder_settings(ForwardProtocolSettings::Socks5(
            Socks5ForwarderSettings::builder()
                .server_address(address)
                .unwrap()
                .build()
                .unwrap(),
        ));
    }

    let settings = builder.build().unwrap();

    let cert_key_file = common::make_cert_key_file();
    let cert_key_path = cert_key_file.path.to_str().unwrap();
    let hosts_settings = TlsHostsSettings::builder()
        .main_hosts(vec![TlsHostInfo {
            hostname: common::MAIN_DOMAIN_NAME.to_string(),
            cert_chain_path: cert_key_path.to_string(),
            private_key_path: cert_key_path.to_string(),
            allowed_sni: vec![],
        }])
        .build()
        .unwrap();

    common::run_endpoint_with_settings(settings, hosts_settings).await;
}

async fn do_connect_request(
    endpoint_address: &SocketAddr,
    proxy_auth: Option<String>,
) -> http::StatusCode {
    let stream =
        common::establish_tls_connection(common::MAIN_DOMAIN_NAME, endpoint_address, None).await;

    let (mut request, conn_driver) = hyper::client::conn::Builder::new()
        .handshake(stream)
        .await
        .unwrap();

    let exchange = async move {
        let mut rr = Request::builder()
            .version(http::Version::HTTP_11)
            .method(http::Method::CONNECT)
            .uri("https://httpbin.agrd.dev:443/");

        if let Some(x) = proxy_auth {
            rr = rr.header(
                http::header::PROXY_AUTHORIZATION,
                format!("Basic {}", BASE64_ENGINE.encode(x)),
            );
        }

        let rr = rr.body(hyper::Body::empty()).unwrap();
        let response = request.send_request(rr).await.unwrap();
        info!("CONNECT response: {:?}", response);
        response.status()
    };

    futures::pin_mut!(conn_driver);
    futures::pin_mut!(exchange);
    match future::select(conn_driver, exchange).await {
        future::Either::Left((_, exchange)) => exchange.await,
        future::Either::Right((x, _)) => x,
    }
}

fn make_socks_server_harness() -> (SocketAddr, impl Future<Output = Vec<u8>>) {
    let server = std::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let _ = server.set_nonblocking(true);
    let server_addr = server.local_addr().unwrap();

    let task = async move {
        let server = TcpListener::from_std(server).unwrap();
        let (mut socket, peer) = server.accept().await.unwrap();
        info!("New connection from {}", peer);

        let mut buf = vec![0; 1024];
        let n = socket.read(&mut buf).await.unwrap();
        assert!(n > 0, "n = {}", n);
        assert_eq!(buf[0], 0x05, "Unexpected version number");
        assert_eq!(buf[1] as usize, n - 2, "Unexpected number of methods");
        Vec::from(&buf[2..n])
    };

    (server_addr, task)
}

fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

fn hs256_token(payload: &str, secret: &str) -> String {
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    use ring::hmac;

    let header = URL_SAFE_NO_PAD.encode(r#"{"alg":"HS256","typ":"JWT"}"#);
    let body = URL_SAFE_NO_PAD.encode(payload);
    let signing_input = format!("{}.{}", header, body);
    let key = hmac::Key::new(hmac::HMAC_SHA256, secret.as_bytes());
    let signature = hmac::sign(&key, signing_input.as_bytes());
    format!(
        "{}.{}",
        signing_input,
        URL_SAFE_NO_PAD.encode(signature.as_ref())
    )
}

#[test]
fn jwt_auth_requires_device_id_claim_by_default_config() {
    std::env::set_var("JWT_SECRET", "secret");
    let auth = JwtAuth::from_config(&JwtAuthConfig {
        algorithm: JwtAlgorithm::HS256,
        issuer: Some("iss".into()),
        audience: Some("aud".into()),
        leeway_seconds: 0,
        username_claim: "sub".into(),
        device_id_claim: Some("device_id".into()),
        metrics_jwt_error_enabled: true,
        public_key_path: None,
        hmac_secret_env: Some("JWT_SECRET".into()),
    })
    .unwrap();

    let ok = hs256_token(
        &format!(
            r#"{{"sub":"alice","exp":{},"iss":"iss","aud":"aud","device_id":"dev-1"}}"#,
            now_unix() + 60
        ),
        "secret",
    );
    assert!(auth.authenticate("alice", &ok).is_ok());

    let no_device = hs256_token(
        &format!(
            r#"{{"sub":"alice","exp":{},"iss":"iss","aud":"aud"}}"#,
            now_unix() + 60
        ),
        "secret",
    );
    assert!(auth.authenticate("alice", &no_device).is_err());
}

#[test]
fn jwt_auth_skips_device_id_claim_if_not_configured() {
    std::env::set_var("JWT_SECRET", "secret");
    let auth = JwtAuth::from_config(&JwtAuthConfig {
        algorithm: JwtAlgorithm::HS256,
        issuer: Some("iss".into()),
        audience: Some("aud".into()),
        leeway_seconds: 0,
        username_claim: "sub".into(),
        device_id_claim: None,
        metrics_jwt_error_enabled: true,
        public_key_path: None,
        hmac_secret_env: Some("JWT_SECRET".into()),
    })
    .unwrap();

    let token = hs256_token(
        &format!(
            r#"{{"sub":"alice","exp":{},"iss":"iss","aud":"aud"}}"#,
            now_unix() + 60
        ),
        "secret",
    );
    assert!(auth.authenticate("alice", &token).is_ok());
}
