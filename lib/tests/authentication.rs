use base64::engine::general_purpose::STANDARD as BASE64_ENGINE;
use base64::Engine;
use futures::future;
use http::Request;
use log::info;
use std::future::Future;
use std::net::{Ipv4Addr, SocketAddr};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::oneshot;
use trusttunnel::authentication;
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
                max_http2_conns: None,
                max_http3_conns: None,
                max_traffic_bytes: None,
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

/// Sends a raw HTTP CONNECT and returns the status, or `None` when the server closes the
/// connection before sending a complete response (also treated as a rejection).
async fn try_connect_raw(
    endpoint_address: &SocketAddr,
    proxy_auth: &str,
    dest: &str,
) -> Option<http::StatusCode> {
    let mut stream =
        common::establish_tls_connection(common::MAIN_DOMAIN_NAME, endpoint_address, None).await;

    let request = format!(
        "CONNECT {dest} HTTP/1.1\r\nHost: {dest}\r\nProxy-Authorization: Basic {}\r\n\r\n",
        BASE64_ENGINE.encode(proxy_auth),
    );
    stream.write_all(request.as_bytes()).await.unwrap();

    let mut response = Vec::new();
    let mut byte = [0u8; 1];
    loop {
        match stream.read_exact(&mut byte).await {
            Ok(_) => {
                response.push(byte[0]);
                if response.ends_with(b"\r\n\r\n") {
                    let line = std::str::from_utf8(&response).unwrap();
                    let code: u16 = line.split_whitespace().nth(1).unwrap().parse().unwrap();
                    return Some(http::StatusCode::from_u16(code).unwrap());
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return None,
            Err(e) => panic!("unexpected error: {e}"),
        }
    }
}

/// Sends a raw HTTP CONNECT over a TLS stream, signals `ready_tx` with the status code as
/// soon as the response headers arrive, then holds the TLS stream open until `release` fires.
/// Using raw I/O avoids hyper closing the connection after the CONNECT 200 exchange, which
/// would cause the server-side tunnel to drop the connection guard prematurely.
async fn connect_and_hold(
    endpoint_address: &SocketAddr,
    proxy_auth: &str,
    dest: &str,
    ready_tx: oneshot::Sender<http::StatusCode>,
    release: oneshot::Receiver<()>,
) {
    let mut stream =
        common::establish_tls_connection(common::MAIN_DOMAIN_NAME, endpoint_address, None).await;

    let request = format!(
        "CONNECT {dest} HTTP/1.1\r\nHost: {dest}\r\nProxy-Authorization: Basic {}\r\n\r\n",
        BASE64_ENGINE.encode(proxy_auth),
    );
    stream.write_all(request.as_bytes()).await.unwrap();

    // Read until the end of the response headers (\r\n\r\n).
    let mut response = Vec::new();
    let mut byte = [0u8; 1];
    loop {
        stream.read_exact(&mut byte).await.unwrap();
        response.push(byte[0]);
        if response.ends_with(b"\r\n\r\n") {
            break;
        }
    }

    let status_str = std::str::from_utf8(&response).unwrap();
    let code: u16 = status_str
        .split_whitespace()
        .nth(1)
        .unwrap()
        .parse()
        .unwrap();
    let _ = ready_tx.send(http::StatusCode::from_u16(code).unwrap());

    // Hold the raw TLS stream open so the server's tunnel stays alive with the guard held.
    let _ = release.await;
    drop(stream);
}

#[tokio::test]
async fn connection_limit_blocks_excess_and_releases_on_disconnect() {
    common::set_up_logger();
    let endpoint_address = common::make_endpoint_address();

    // A local TCP listener acts as a stable tunnel destination so the server's pipe
    // keeps running and the connection slot stays held.
    let dest_listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0u16))
        .await
        .unwrap();
    let dest_addr = format!("127.0.0.1:{}", dest_listener.local_addr().unwrap().port());

    let (release_tx, release_rx) = oneshot::channel::<()>();
    let (ready_tx, ready_rx) = oneshot::channel::<http::StatusCode>();

    let conn1_task = {
        let addr = endpoint_address;
        let dest = dest_addr.clone();
        async move {
            connect_and_hold(&addr, "a:b", &dest, ready_tx, release_rx).await;
        }
    };

    let test_task = async move {
        // Give the endpoint time to start.
        tokio::time::sleep(Duration::from_millis(1200)).await;

        // conn1: open and hold (slot = 1/1)
        tokio::spawn(conn1_task);

        // Wait until conn1's CONNECT has been processed and slot is held.
        let status1 = ready_rx.await.unwrap();
        assert_ne!(
            status1,
            http::StatusCode::PROXY_AUTHENTICATION_REQUIRED,
            "conn1 must succeed"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;

        // conn2: should be denied — limit already reached.
        // The server may send 407 or drop the connection without a response (if the
        // response buffer isn't flushed before the tunnel drops), so both count as rejection.
        let result2 = try_connect_raw(&endpoint_address, "a:b", "127.0.0.1:1").await;
        assert!(
            result2.is_none() || result2 == Some(http::StatusCode::PROXY_AUTHENTICATION_REQUIRED),
            "conn2 must be rejected while limit is held, got: {:?}",
            result2
        );

        // Release conn1 and give the server time to free the slot.
        let _ = release_tx.send(());
        tokio::time::sleep(Duration::from_millis(300)).await;

        // conn3: slot released — must succeed now.
        let status3 = do_connect_request(&endpoint_address, Some("a:b".into())).await;
        assert_ne!(
            status3,
            http::StatusCode::PROXY_AUTHENTICATION_REQUIRED,
            "conn3 must succeed after slot is released"
        );

        drop(dest_listener);
    };

    tokio::select! {
        _ = run_endpoint_with_conn_limit(&endpoint_address, 1) => unreachable!(),
        _ = test_task => (),
        _ = tokio::time::sleep(Duration::from_secs(15)) => panic!("Timed out"),
    }
}

async fn run_endpoint_with_conn_limit(listen_address: &SocketAddr, max_http2_conns: u32) {
    let settings = Settings::builder()
        .listen_address(listen_address)
        .unwrap()
        .listen_protocols(ListenProtocolSettings {
            http1: Some(Http1Settings::builder().build()),
            ..Default::default()
        })
        .allow_private_network_connections(true)
        .clients(vec![authentication::registry_based::Client {
            username: "a".into(),
            password: "b".into(),
            max_http2_conns: Some(max_http2_conns),
            max_http3_conns: None,
            max_traffic_bytes: None,
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
