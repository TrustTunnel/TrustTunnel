use bytes::Bytes;
use futures::stream;
use http::{Request, Response};
use log::info;
use once_cell::sync::Lazy;
use ring::digest::{digest, SHA256};
use std::future::Future;
use std::net::{Ipv4Addr, SocketAddr};
use std::time::Duration;
use tokio::net::TcpListener;
use trusttunnel::net_utils;
use trusttunnel::settings::{
    Http1Settings, Http2Settings, ListenProtocolSettings, QuicSettings, ReverseProxySettings,
    Settings, TlsHostInfo, TlsHostsSettings,
};

#[allow(dead_code)]
mod common;

// Use a larger body to catch partial responses without flooding logs.
static RESPONSE_BODY: Lazy<Bytes> = Lazy::new(|| Bytes::from(vec![b'x'; 1024 * 1024]));

macro_rules! reverse_proxy_tests {
    ($($name:ident: $client_fn:expr,)*) => {
    $(
        #[tokio::test]
        async fn $name() {
            common::set_up_logger();
            let endpoint_address = common::make_endpoint_address();
            let (proxy_address, proxy_task) = run_proxy();

            let client_task = async {
                tokio::time::sleep(Duration::from_secs(1)).await;
                let (response, body) = $client_fn(&endpoint_address).await;
                assert_eq!(response.status, http::StatusCode::OK);
                assert_body_matches(&body);
            };

            // Pin both tasks to avoid moving them
            tokio::pin!(client_task);
            tokio::pin!(proxy_task);

            tokio::select! {
                _ = run_endpoint(&endpoint_address, &proxy_address) => unreachable!(),
                _ = tokio::time::sleep(Duration::from_secs(10)) => panic!("Timed out"),
                // Wait for client_task first; if proxy_task completes, continue waiting for client
                _ = &mut client_task => (),
                _ = &mut proxy_task => {
                    // Proxy completed (expected after handling request), now wait for client
                    tokio::select! {
                        _ = client_task => (),
                        _ = tokio::time::sleep(Duration::from_secs(5)) => panic!("Client timed out after proxy completed"),
                    }
                },
            }
        }
    )*
    }
}

reverse_proxy_tests! {
    sni_h1: sni_h1_client,
    sni_h3: sni_h3_client,
    path_h1: path_h1_client,
    path_h2: path_h2_client,
    path_h3: path_h3_client,
}

#[tokio::test]
async fn path_h2_chunked() {
    common::set_up_logger();
    let endpoint_address = common::make_endpoint_address();
    let (proxy_address, proxy_task) = run_proxy_chunked();

    let client_task = async {
        tokio::time::sleep(Duration::from_secs(1)).await;
        let (response, body) = path_h2_client(&endpoint_address).await;
        assert_eq!(response.status, http::StatusCode::OK);
        assert_body_matches(&body);
    };

    tokio::pin!(client_task);
    tokio::pin!(proxy_task);

    tokio::select! {
        _ = run_endpoint(&endpoint_address, &proxy_address) => unreachable!(),
        _ = tokio::time::sleep(Duration::from_secs(10)) => panic!("Timed out"),
        _ = &mut client_task => (),
        _ = &mut proxy_task => {
            tokio::select! {
                _ = client_task => (),
                _ = tokio::time::sleep(Duration::from_secs(5)) => {
                    panic!("Client timed out after proxy completed")
                }
            }
        },
    }
}

#[tokio::test]
async fn path_h3_chunked() {
    common::set_up_logger();
    let endpoint_address = common::make_endpoint_address();
    let (proxy_address, proxy_task) = run_proxy_chunked();

    let client_task = async {
        tokio::time::sleep(Duration::from_secs(1)).await;
        let (response, body) = path_h3_client(&endpoint_address).await;
        assert_eq!(response.status, http::StatusCode::OK);
        assert_body_matches(&body);
    };

    tokio::pin!(client_task);
    tokio::pin!(proxy_task);

    tokio::select! {
        _ = run_endpoint(&endpoint_address, &proxy_address) => unreachable!(),
        _ = tokio::time::sleep(Duration::from_secs(10)) => panic!("Timed out"),
        _ = &mut client_task => (),
        _ = &mut proxy_task => {
            tokio::select! {
                _ = client_task => (),
                _ = tokio::time::sleep(Duration::from_secs(5)) => {
                    panic!("Client timed out after proxy completed")
                }
            }
        },
    }
}

fn assert_body_matches(body: &Bytes) {
    assert_eq!(body.len(), RESPONSE_BODY.len(), "response length mismatch");
    let expected_hash = digest(&SHA256, RESPONSE_BODY.as_ref());
    let actual_hash = digest(&SHA256, body.as_ref());
    assert_eq!(
        actual_hash.as_ref(),
        expected_hash.as_ref(),
        "response hash mismatch"
    );
}

async fn sni_h1_client(endpoint_address: &SocketAddr) -> (http::response::Parts, Bytes) {
    let stream = common::establish_tls_connection(
        &format!("hello.{}", common::MAIN_DOMAIN_NAME),
        endpoint_address,
        None,
    )
    .await;

    common::do_get_request(
        stream,
        http::Version::HTTP_11,
        &format!(
            "https://hello.{}:{}",
            common::MAIN_DOMAIN_NAME,
            endpoint_address.port()
        ),
        &[],
    )
    .await
}

async fn sni_h3_client(endpoint_address: &SocketAddr) -> (http::response::Parts, Bytes) {
    let mut conn = common::Http3Session::connect(
        endpoint_address,
        &format!("hello.{}", common::MAIN_DOMAIN_NAME),
        None,
    )
    .await;

    conn.exchange(
        Request::get(format!(
            "https://hello.{}:{}",
            common::MAIN_DOMAIN_NAME,
            endpoint_address.port()
        ))
        .body(hyper::Body::empty())
        .unwrap(),
    )
    .await
}

async fn path_h1_client(endpoint_address: &SocketAddr) -> (http::response::Parts, Bytes) {
    let stream =
        common::establish_tls_connection(common::MAIN_DOMAIN_NAME, endpoint_address, None).await;

    common::do_get_request(
        stream,
        http::Version::HTTP_11,
        &format!(
            "https://{}:{}/hello/haha",
            common::MAIN_DOMAIN_NAME,
            endpoint_address.port()
        ),
        &[(http::header::UPGRADE.as_str(), "1")],
    )
    .await
}

async fn path_h2_client(endpoint_address: &SocketAddr) -> (http::response::Parts, Bytes) {
    let stream = common::establish_tls_connection(
        common::MAIN_DOMAIN_NAME,
        endpoint_address,
        Some(net_utils::HTTP2_ALPN.as_bytes()),
    )
    .await;

    common::do_get_request(
        stream,
        http::Version::HTTP_2,
        &format!(
            "https://{}:{}/hello/haha",
            common::MAIN_DOMAIN_NAME,
            endpoint_address.port()
        ),
        &[],
    )
    .await
}

async fn path_h3_client(endpoint_address: &SocketAddr) -> (http::response::Parts, Bytes) {
    let mut conn =
        common::Http3Session::connect(endpoint_address, common::MAIN_DOMAIN_NAME, None).await;

    conn.exchange(
        Request::get(format!(
            "https://{}:{}/hello/haha",
            common::MAIN_DOMAIN_NAME,
            endpoint_address.port()
        ))
        .body(hyper::Body::empty())
        .unwrap(),
    )
    .await
}

async fn run_endpoint(endpoint_address: &SocketAddr, proxy_address: &SocketAddr) {
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
                .path_mask("/hello".to_string())
                .build()
                .unwrap(),
        )
        .allow_private_network_connections(true)
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
        .reverse_proxy_hosts(vec![TlsHostInfo {
            hostname: format!("hello.{}", common::MAIN_DOMAIN_NAME),
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

fn run_proxy_chunked() -> (SocketAddr, impl Future<Output = ()>) {
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
            .serve_connection(
                socket,
                hyper::service::service_fn(request_handler_chunked),
            )
            .await
            .unwrap();
    })
}

async fn request_handler(
    request: Request<hyper::Body>,
) -> Result<Response<hyper::Body>, hyper::Error> {
    info!("Received request: {:?}", request);
    Ok(Response::builder()
        .body(hyper::Body::from(RESPONSE_BODY.clone()))
        .unwrap())
}

async fn request_handler_chunked(
    request: Request<hyper::Body>,
) -> Result<Response<hyper::Body>, hyper::Error> {
    info!("Received request: {:?}", request);
    let chunk_size = 16 * 1024;
    let chunks = RESPONSE_BODY
        .chunks(chunk_size)
        .map(|c| Ok::<Bytes, std::io::Error>(Bytes::copy_from_slice(c)))
        .collect::<Vec<_>>();

    Ok(Response::builder()
        .body(hyper::Body::wrap_stream(stream::iter(chunks)))
        .unwrap())
}
