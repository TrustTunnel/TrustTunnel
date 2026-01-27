use bytes::Bytes;
use futures::future;
use http::{Request, StatusCode};
use log::info;
use std::future::Future;
use std::net::{Ipv4Addr, SocketAddr};
use std::time::Duration;
use tokio::net::TcpListener;
use trusttunnel::authentication::{registry_based::Client, tunnel_token_from_credentials};
use trusttunnel::settings::{
    Http1Settings, Http2Settings, ListenProtocolSettings, QuicSettings, ReverseProxySettings,
    Settings, TlsHostInfo, TlsHostsSettings,
};

#[allow(dead_code)]
mod common;

const PING_PATH: &str = "/ping";
const SPEEDTEST_PATH: &str = "/speedtest";
const TUNNEL_USER: &str = "user";
const TUNNEL_PASS: &str = "pass";

#[tokio::test]
async fn ping_path_h1() {
    common::set_up_logger();
    let endpoint_address = common::make_endpoint_address();
    let (proxy_address, proxy_task) = run_proxy();

    let client_task = async {
        tokio::time::sleep(Duration::from_secs(1)).await;
        let (status, body) = ping_path_h1_client(&endpoint_address).await;
        assert_eq!(status, StatusCode::OK);
        assert!(body.is_empty());
    };

    tokio::pin!(client_task);
    tokio::pin!(proxy_task);

    tokio::select! {
        _ = run_endpoint_with_paths(&endpoint_address, &proxy_address, false) => unreachable!(),
        _ = tokio::time::sleep(Duration::from_secs(10)) => panic!("Timed out"),
        _ = &mut client_task => (),
        _ = &mut proxy_task => {
            tokio::select! {
                _ = client_task => (),
                _ = tokio::time::sleep(Duration::from_secs(5)) => panic!("Client timed out after proxy completed"),
            }
        },
    }
}

#[tokio::test]
async fn speedtest_path_h1() {
    common::set_up_logger();
    let endpoint_address = common::make_endpoint_address();
    let (proxy_address, proxy_task) = run_proxy();

    let client_task = async {
        tokio::time::sleep(Duration::from_secs(1)).await;
        let (status, body) = speedtest_path_h1_client(&endpoint_address).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body.len(), 1 * 1024 * 1024);
    };

    tokio::pin!(client_task);
    tokio::pin!(proxy_task);

    tokio::select! {
        _ = run_endpoint_with_paths(&endpoint_address, &proxy_address, false) => unreachable!(),
        _ = tokio::time::sleep(Duration::from_secs(20)) => panic!("Timed out"),
        _ = &mut client_task => (),
        _ = &mut proxy_task => {
            tokio::select! {
                _ = client_task => (),
                _ = tokio::time::sleep(Duration::from_secs(5)) => panic!("Client timed out after proxy completed"),
            }
        },
    }
}

#[tokio::test]
async fn tunnel_token_h1() {
    common::set_up_logger();
    let endpoint_address = common::make_endpoint_address();
    let (proxy_address, proxy_task) = run_proxy();

    let client_task = async {
        tokio::time::sleep(Duration::from_secs(1)).await;
        let (status, _body) = tunnel_with_token_h1_client(&endpoint_address).await;
        assert_eq!(status, StatusCode::PROXY_AUTHENTICATION_REQUIRED);
    };

    tokio::pin!(client_task);
    tokio::pin!(proxy_task);

    tokio::select! {
        _ = run_endpoint_with_paths(&endpoint_address, &proxy_address, false) => unreachable!(),
        _ = tokio::time::sleep(Duration::from_secs(10)) => panic!("Timed out"),
        _ = &mut client_task => (),
        _ = &mut proxy_task => {
            tokio::select! {
                _ = client_task => (),
                _ = tokio::time::sleep(Duration::from_secs(5)) => panic!("Client timed out after proxy completed"),
            }
        },
    }
}

#[tokio::test]
async fn tunnel_legacy_without_token_h1() {
    common::set_up_logger();
    let endpoint_address = common::make_endpoint_address();
    let (proxy_address, proxy_task) = run_proxy();

    let client_task = async {
        tokio::time::sleep(Duration::from_secs(1)).await;
        let status = tunnel_without_token_h1_client(&endpoint_address).await;
        assert_eq!(status, StatusCode::PROXY_AUTHENTICATION_REQUIRED);
    };

    tokio::pin!(client_task);
    tokio::pin!(proxy_task);

    tokio::select! {
        _ = run_endpoint_with_paths(&endpoint_address, &proxy_address, true) => unreachable!(),
        _ = tokio::time::sleep(Duration::from_secs(10)) => panic!("Timed out"),
        _ = &mut client_task => (),
        _ = &mut proxy_task => {
            tokio::select! {
                _ = client_task => (),
                _ = tokio::time::sleep(Duration::from_secs(5)) => panic!("Client timed out after proxy completed"),
            }
        },
    }
}

async fn ping_path_h1_client(endpoint_address: &SocketAddr) -> (StatusCode, Bytes) {
    let stream =
        common::establish_tls_connection(common::MAIN_DOMAIN_NAME, endpoint_address, None).await;

    let (response, body) = common::do_get_request(
        stream,
        http::Version::HTTP_11,
        &format!(
            "https://{}:{}{}/health",
            common::MAIN_DOMAIN_NAME,
            endpoint_address.port(),
            PING_PATH
        ),
        &[],
    )
    .await;

    (response.status, body)
}

async fn speedtest_path_h1_client(endpoint_address: &SocketAddr) -> (StatusCode, Bytes) {
    let stream =
        common::establish_tls_connection(common::MAIN_DOMAIN_NAME, endpoint_address, None).await;

    let (response, body) = common::do_get_request(
        stream,
        http::Version::HTTP_11,
        &format!(
            "https://{}:{}{}/1mb.bin",
            common::MAIN_DOMAIN_NAME,
            endpoint_address.port(),
            SPEEDTEST_PATH
        ),
        &[],
    )
    .await;

    (response.status, body)
}

async fn tunnel_with_token_h1_client(endpoint_address: &SocketAddr) -> (StatusCode, Bytes) {
    let stream =
        common::establish_tls_connection(common::MAIN_DOMAIN_NAME, endpoint_address, None).await;
    let token = tunnel_token_from_credentials(TUNNEL_USER, TUNNEL_PASS);

    let (response, body) = common::do_get_request(
        stream,
        http::Version::HTTP_11,
        &format!(
            "https://{}:{}/proxy",
            common::MAIN_DOMAIN_NAME,
            endpoint_address.port(),
        ),
        &[("x-tunnel-token", token.as_str())],
    )
    .await;

    (response.status, body)
}

async fn tunnel_without_token_h1_client(endpoint_address: &SocketAddr) -> StatusCode {
    let stream =
        common::establish_tls_connection(common::MAIN_DOMAIN_NAME, endpoint_address, None).await;

    let (mut request, conn) = hyper::client::conn::Builder::new()
        .handshake(stream)
        .await
        .unwrap();

    let exchange = async move {
        let rr = Request::builder()
            .version(http::Version::HTTP_11)
            .method(http::Method::CONNECT)
            .uri(format!("{}:{}", common::MAIN_DOMAIN_NAME, endpoint_address.port()))
            .body(hyper::Body::empty())
            .unwrap();
        let response = request.send_request(rr).await.unwrap();
        response.status()
    };

    futures::pin_mut!(exchange);
    match future::select(conn, exchange).await {
        future::Either::Left((r, exchange)) => {
            info!("HTTP connection closed with result: {:?}", r);
            exchange.await
        }
        future::Either::Right((status, _)) => status,
    }
}

async fn run_endpoint_with_paths(
    endpoint_address: &SocketAddr,
    proxy_address: &SocketAddr,
    allow_without_token: bool,
) {
    let settings = Settings::builder()
        .listen_address(endpoint_address)
        .unwrap()
        .listen_protocols(ListenProtocolSettings {
            http1: Some(Http1Settings::builder().build()),
            http2: Some(Http2Settings::builder().build()),
            quic: Some(QuicSettings::builder().build()),
        })
        .reverse_proxy(
            ReverseProxySettings::builder()
                .server_address(proxy_address)
                .unwrap()
                .path_mask("/proxy".to_string())
                .build()
                .unwrap(),
        )
        .allow_private_network_connections(true)
        .speedtest_enable(true)
        .ping_enable(true)
        .ping_path(PING_PATH)
        .speedtest_path(SPEEDTEST_PATH)
        .allow_without_token(allow_without_token)
        .clients(vec![Client {
            username: TUNNEL_USER.to_string(),
            password: TUNNEL_PASS.to_string(),
        }])
        .build()
        .unwrap();

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

fn run_proxy() -> (SocketAddr, impl Future<Output = ()>) {
    let server = std::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let _ = server.set_nonblocking(true);
    let server_addr = server.local_addr().unwrap();
    (server_addr, async move {
        let (socket, peer) = TcpListener::from_std(server)
            .unwrap()
            .accept()
            .await
            .unwrap();
        info!("New connection from {}", peer);
        hyper::server::conn::Http::new()
            .http1_only(true)
            .serve_connection(socket, hyper::service::service_fn(request_handler))
            .await
            .unwrap();
    })
}

async fn request_handler(
    request: Request<hyper::Body>,
) -> Result<http::Response<hyper::Body>, hyper::Error> {
    info!("Received request: {:?}", request);
    Ok(http::Response::builder()
        .body(hyper::Body::from("proxy-ok"))
        .unwrap())
}
