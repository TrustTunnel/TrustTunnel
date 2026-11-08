use bytes::BufMut;
use futures::{future, FutureExt, StreamExt};
use http::Request;
use log::info;
use std::future::Future;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::pin::Pin;
use std::thread;
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream, UdpSocket};
use trusttunnel::net_utils;
use trusttunnel::settings::{
    ForwardProtocolSettings, Http2Settings, ListenProtocolSettings, Settings,
    Socks5ForwarderSettings, TlsHostInfo, TlsHostsSettings,
};

#[allow(dead_code)]
mod common;

const TCP_CONTENT_SIZE: usize = 2 * 1024 * 1024;
const UDP_CHUNK_SIZE: usize = 1024;
const UDP_CONTENT_SIZE: usize = 8 * UDP_CHUNK_SIZE;
const MANGLED_UDP_HEADER_LENGTH: usize = 4 + 2 * (16 + 2);
const EXPECTED_MANGLED_UDP_LENGTH: usize =
    UDP_CONTENT_SIZE + (UDP_CONTENT_SIZE / UDP_CHUNK_SIZE) * MANGLED_UDP_HEADER_LENGTH;

macro_rules! tcp_download_tests {
    ($($name:ident: $make_tunnel_fn:expr,)*) => {
    $(
        #[tokio::test]
        async fn $name() {
            common::set_up_logger();
            let endpoint_address = common::make_endpoint_address();

            let client_task = async {
                let server_address = run_tcp_server(true);
                tokio::time::sleep(Duration::from_secs(1)).await;

                let (conn_driver, io) = $make_tunnel_fn(endpoint_address, server_address.to_string()).await;

                let exchange = async {
                    let mut io = io.await;
                    let mut total = 0;
                    let mut buf = [0; 64 * 1024];
                    while total < TCP_CONTENT_SIZE {
                        match io.read(&mut buf).await.unwrap() {
                            0 => break,
                            n => total += n,
                        }
                    }
                    assert_eq!(total, TCP_CONTENT_SIZE);
                };

                futures::pin_mut!(exchange);
                match future::select(conn_driver, exchange).await {
                    future::Either::Left((r, exchange)) => {
                        info!("HTTP connection closed with result: {:?}", r);
                        exchange.await
                    }
                    future::Either::Right(_) => (),
                }
            };

            tokio::select! {
                _ = common::run_endpoint(&endpoint_address) => unreachable!(),
                _ = client_task => (),
                _ = tokio::time::sleep(Duration::from_secs(10)) => panic!("Timed out"),
            }
        }
    )*
    }
}

macro_rules! tcp_upload_tests {
    ($($name:ident: $make_tunnel_fn:expr,)*) => {
    $(
        #[tokio::test]
        async fn $name() {
            common::set_up_logger();
            let endpoint_address = common::make_endpoint_address();

            let client_task = async {
                let server_address = run_tcp_server(false);
                tokio::time::sleep(Duration::from_secs(1)).await;

                let (conn_driver, io) = $make_tunnel_fn(endpoint_address, server_address.to_string()).await;

                let exchange = async {
                    let mut io = io.await;
                    let mut content = common::make_stream_of_chunks(TCP_CONTENT_SIZE, None);
                    while let Some(chunk) = content.next().await {
                        io.write_all(chunk).await.unwrap();
                    }
                    io.flush().await.unwrap();

                    let mut ack = [0; 1];
                    assert_eq!(io.read(&mut ack).await.unwrap(), 1);
                };

                futures::pin_mut!(exchange);
                match future::select(conn_driver, exchange).await {
                    future::Either::Left((r, exchange)) => {
                        info!("HTTP connection closed with result: {:?}", r);
                        exchange.await
                    }
                    future::Either::Right(_) => (),
                }
            };

            tokio::select! {
                _ = common::run_endpoint(&endpoint_address) => unreachable!(),
                _ = client_task => (),
                _ = tokio::time::sleep(Duration::from_secs(10)) => panic!("Timed out"),
            }
        }
    )*
    }
}

tcp_download_tests! {
    h1_tcp_download: make_h1_tunnel,
    h2_tcp_download: make_h2_tunnel,
}

tcp_upload_tests! {
    h1_tcp_upload: make_h1_tunnel,
    h2_tcp_upload: make_h2_tunnel,
}

#[tokio::test]
async fn h2_udp_download() {
    common::set_up_logger();
    let endpoint_address = common::make_endpoint_address();

    let client_task = async {
        let server_address = run_udp_server(true);
        tokio::time::sleep(Duration::from_secs(1)).await;

        let (conn_driver, io) = make_h2_tunnel(endpoint_address, "_udp2".to_string()).await;

        let exchange = async {
            let mut io = io.await;
            let hole_puncher = encode_udp_chunk(&server_address, &[1]);
            io.write_all(&hole_puncher).await.unwrap();

            let mut total = 0;
            let mut buf = [0; 64 * 1024];
            while total < EXPECTED_MANGLED_UDP_LENGTH {
                match io.read(&mut buf).await.unwrap() {
                    0 => break,
                    n => total += n,
                }
            }
            assert_eq!(total, EXPECTED_MANGLED_UDP_LENGTH);
        };

        futures::pin_mut!(exchange);
        match future::select(conn_driver, exchange).await {
            future::Either::Left((r, exchange)) => {
                info!("HTTP connection closed with result: {:?}", r);
                exchange.await
            }
            future::Either::Right(_) => (),
        }
    };

    tokio::select! {
        _ = common::run_endpoint(&endpoint_address) => unreachable!(),
        _ = client_task => (),
        _ = tokio::time::sleep(Duration::from_secs(10)) => panic!("Timed out"),
    }
}

#[tokio::test]
async fn h2_udp_upload() {
    common::set_up_logger();
    let endpoint_address = common::make_endpoint_address();

    let client_task = async {
        let server_address = run_udp_server(false);
        tokio::time::sleep(Duration::from_secs(1)).await;

        let (conn_driver, io) = make_h2_tunnel(endpoint_address, "_udp2".to_string()).await;

        let exchange = async {
            let mut io = io.await;

            let mut content = common::make_stream_of_chunks(UDP_CONTENT_SIZE, Some(UDP_CHUNK_SIZE))
                .map(|x| encode_udp_chunk(&server_address, x));
            while let Some(chunk) = content.next().await {
                io.write_all(&chunk).await.unwrap();
            }

            let mut ack = [0; UDP_CHUNK_SIZE];
            assert_eq!(
                io.read(&mut ack).await.unwrap(),
                MANGLED_UDP_HEADER_LENGTH + 1
            );
        };

        futures::pin_mut!(exchange);
        match future::select(conn_driver, exchange).await {
            future::Either::Left((r, exchange)) => {
                info!("HTTP connection closed with result: {:?}", r);
                exchange.await
            }
            future::Either::Right(_) => (),
        }
    };

    tokio::select! {
        _ = common::run_endpoint(&endpoint_address) => unreachable!(),
        _ = client_task => (),
        _ = tokio::time::sleep(Duration::from_secs(10)) => panic!("Timed out"),
    }
}

#[tokio::test]
async fn h2_udp_socks5_multiple_associations() {
    common::set_up_logger();
    let endpoint_address = common::make_endpoint_address();
    let socks_address = run_socks5_udp_echo_server();
    let destination = SocketAddr::from((Ipv4Addr::LOCALHOST, 5353));

    let client_task = async {
        tokio::time::sleep(Duration::from_secs(1)).await;

        let (conn_driver, io) = make_h2_tunnel(endpoint_address, "_udp2".to_string()).await;

        let exchange = async {
            let mut io = io.await;
            for round in 0..2_u8 {
                for source_port in [1234, 1235, 1236] {
                    let chunk = encode_udp_chunk_from_port(source_port, &destination, &[round]);
                    io.write_all(&chunk).await.unwrap();

                    let reply = read_udp_reply(&mut io).await;
                    assert_eq!(reply_destination_port(&reply), source_port);
                    assert_eq!(&reply[MANGLED_UDP_HEADER_LENGTH..], &[round]);
                }
            }
        };

        futures::pin_mut!(exchange);
        match future::select(conn_driver, exchange).await {
            future::Either::Left((r, exchange)) => {
                info!("HTTP connection closed with result: {:?}", r);
                exchange.await
            }
            future::Either::Right(_) => (),
        }
    };

    tokio::select! {
        _ = run_endpoint_with_socks5_forwarder(&endpoint_address, socks_address) => unreachable!(),
        _ = client_task => (),
        _ = tokio::time::sleep(Duration::from_secs(10)) => panic!("Timed out"),
    }
}

#[tokio::test]
async fn h3_tcp_download() {
    common::set_up_logger();
    let endpoint_address = common::make_endpoint_address();

    let client_task = async {
        let server_address = run_tcp_server(true);
        tokio::time::sleep(Duration::from_secs(1)).await;

        let mut conn =
            common::Http3Session::connect(&endpoint_address, common::MAIN_DOMAIN_NAME, None).await;

        let (response, _) = conn
            .exchange(
                Request::connect(server_address.to_string())
                    .body(hyper::Body::empty())
                    .unwrap(),
            )
            .await;
        assert_eq!(response.status, http::StatusCode::OK);

        let mut total = 0;
        let mut buf = [0; 64 * 1024];
        while total < TCP_CONTENT_SIZE {
            match conn.recv(&mut buf).await {
                0 => break,
                n => total += n,
            }
        }
        assert_eq!(total, TCP_CONTENT_SIZE);
    };

    tokio::select! {
        _ = common::run_endpoint(&endpoint_address) => unreachable!(),
        _ = client_task => (),
        _ = tokio::time::sleep(Duration::from_secs(10)) => panic!("Timed out"),
    }
}

#[tokio::test]
async fn h3_tcp_upload() {
    common::set_up_logger();
    let endpoint_address = common::make_endpoint_address();

    let client_task = async {
        let server_address = run_tcp_server(false);
        tokio::time::sleep(Duration::from_secs(1)).await;

        let mut conn =
            common::Http3Session::connect(&endpoint_address, common::MAIN_DOMAIN_NAME, None).await;

        let (response, _) = conn
            .exchange(
                Request::connect(server_address.to_string())
                    .body(hyper::Body::empty())
                    .unwrap(),
            )
            .await;
        assert_eq!(response.status, http::StatusCode::OK);

        conn.send(common::make_stream_of_chunks(TCP_CONTENT_SIZE, None))
            .await;
        let mut ack = [0; 1];
        assert_eq!(conn.recv(&mut ack).await, 1);
    };

    tokio::select! {
        _ = common::run_endpoint(&endpoint_address) => unreachable!(),
        _ = client_task => (),
        _ = tokio::time::sleep(Duration::from_secs(10)) => panic!("Timed out"),
    }
}

#[tokio::test]
async fn h3_udp_download() {
    common::set_up_logger();
    let endpoint_address = common::make_endpoint_address();

    let client_task = async {
        let server_address = run_udp_server(true);
        tokio::time::sleep(Duration::from_secs(1)).await;

        let mut conn =
            common::Http3Session::connect(&endpoint_address, common::MAIN_DOMAIN_NAME, None).await;

        let (response, _) = conn
            .exchange(
                Request::connect("_udp2")
                    .body(hyper::Body::empty())
                    .unwrap(),
            )
            .await;
        assert_eq!(response.status, http::StatusCode::OK);

        let hole_puncher = encode_udp_chunk(&server_address, &[1]);
        conn.send(futures::stream::iter(std::iter::once(hole_puncher)))
            .await;

        let mut total = 0;
        let mut buf = [0; 64 * 1024];
        while total < EXPECTED_MANGLED_UDP_LENGTH {
            match conn.recv(&mut buf).await {
                0 => break,
                n => total += n,
            }
        }
        assert_eq!(total, EXPECTED_MANGLED_UDP_LENGTH);
    };

    tokio::select! {
        _ = common::run_endpoint(&endpoint_address) => unreachable!(),
        _ = client_task => (),
        _ = tokio::time::sleep(Duration::from_secs(10)) => panic!("Timed out"),
    }
}

#[tokio::test]
async fn h3_udp_upload() {
    common::set_up_logger();
    let endpoint_address = common::make_endpoint_address();

    let client_task = async {
        let server_address = run_udp_server(false);
        tokio::time::sleep(Duration::from_secs(1)).await;

        let mut conn =
            common::Http3Session::connect(&endpoint_address, common::MAIN_DOMAIN_NAME, None).await;

        let (response, _) = conn
            .exchange(
                Request::connect("_udp2")
                    .body(hyper::Body::empty())
                    .unwrap(),
            )
            .await;
        assert_eq!(response.status, http::StatusCode::OK);

        conn.send(
            common::make_stream_of_chunks(UDP_CONTENT_SIZE, Some(UDP_CHUNK_SIZE))
                .map(|x| encode_udp_chunk(&server_address, x)),
        )
        .await;

        let mut ack = [0; UDP_CHUNK_SIZE];
        assert_eq!(conn.recv(&mut ack).await, MANGLED_UDP_HEADER_LENGTH + 1);
    };

    tokio::select! {
        _ = common::run_endpoint(&endpoint_address) => unreachable!(),
        _ = client_task => (),
        _ = tokio::time::sleep(Duration::from_secs(10)) => panic!("Timed out"),
    }
}

async fn make_h1_tunnel(
    endpoint_address: SocketAddr,
    server_address: String,
) -> (
    Pin<Box<dyn Future<Output = ()>>>,
    Pin<Box<dyn Future<Output = impl AsyncRead + AsyncWrite + Unpin + Send>>>,
) {
    let stream =
        common::establish_tls_connection(common::MAIN_DOMAIN_NAME, &endpoint_address, None).await;

    let (mut request, conn) = hyper::client::conn::Builder::new()
        .handshake(stream)
        .await
        .unwrap();

    let conn_driver = async move { conn.await.unwrap() }.boxed();

    let exchange = async move {
        let rr = Request::builder()
            .version(http::Version::HTTP_11)
            .method(http::Method::CONNECT)
            .uri(server_address)
            .body(hyper::Body::empty())
            .unwrap();
        let response = request.send_request(rr).await.unwrap();
        info!("CONNECT response: {:?}", response);
        assert_eq!(response.status(), http::StatusCode::OK);

        hyper::upgrade::on(response).await.unwrap()
    }
    .boxed();

    (conn_driver, exchange)
}

async fn make_h2_tunnel(
    endpoint_address: SocketAddr,
    server_address: String,
) -> (
    Pin<Box<dyn Future<Output = ()>>>,
    Pin<Box<dyn Future<Output = impl AsyncRead + AsyncWrite + Unpin + Send>>>,
) {
    let stream = common::establish_tls_connection(
        common::MAIN_DOMAIN_NAME,
        &endpoint_address,
        Some(net_utils::HTTP2_ALPN.as_bytes()),
    )
    .await;

    let (mut request, conn) = hyper::client::conn::Builder::new()
        .http2_only(true)
        .handshake(stream)
        .await
        .unwrap();

    let conn_driver = async move { conn.await.unwrap() }.boxed();

    let exchange = async move {
        let rr = Request::builder()
            .version(http::Version::HTTP_2)
            .method(http::Method::CONNECT)
            .uri(server_address)
            .body(hyper::Body::empty())
            .unwrap();
        let response = request.send_request(rr).await.unwrap();
        info!("CONNECT response: {:?}", response);
        assert_eq!(response.status(), http::StatusCode::OK);

        hyper::upgrade::on(response).await.unwrap()
    }
    .boxed();

    (conn_driver, exchange)
}

fn run_tcp_server(is_download: bool) -> SocketAddr {
    let server = std::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let _ = server.set_nonblocking(true);
    let server_addr = server.local_addr().unwrap();

    thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async move {
            let server = TcpListener::from_std(server).unwrap();
            let (mut socket, peer) = server.accept().await.unwrap();
            info!("New connection from {}", peer);

            if is_download {
                let mut content = common::make_stream_of_chunks(TCP_CONTENT_SIZE, None);
                while let Some(chunk) = content.next().await {
                    socket.write_all(chunk).await.unwrap();
                }
            } else {
                let mut total = 0;
                let mut buf = [0; 64 * 1024];
                while total < TCP_CONTENT_SIZE {
                    match socket.read(&mut buf).await.unwrap() {
                        0 => break,
                        n => total += n,
                    }
                }

                assert_eq!(total, TCP_CONTENT_SIZE);
                let ack = 1_u8;
                socket.write_all(&[ack]).await.unwrap();
            }

            socket.flush().await.unwrap();
        });
    });

    server_addr
}

fn run_udp_server(is_download: bool) -> SocketAddr {
    let server = std::net::UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let _ = server.set_nonblocking(true);
    let server_addr = server.local_addr().unwrap();

    thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async move {
            let server = UdpSocket::from_std(server).unwrap();
            if is_download {
                let mut buf = [0; UDP_CHUNK_SIZE];
                let (n, peer) = server.recv_from(&mut buf).await.unwrap();
                assert_eq!(n, 1);

                let mut content =
                    common::make_stream_of_chunks(UDP_CONTENT_SIZE, Some(UDP_CHUNK_SIZE));
                while let Some(chunk) = content.next().await {
                    server.send_to(chunk, peer).await.unwrap();
                }
            } else {
                let mut peer = None;
                let mut total = 0;
                let mut buf = [0; UDP_CHUNK_SIZE];
                while total < UDP_CONTENT_SIZE {
                    let (n, p) = server.recv_from(&mut buf).await.unwrap();
                    assert_eq!(*peer.get_or_insert(p), p);
                    total += n;
                }

                assert_eq!(total, UDP_CONTENT_SIZE);
                let ack = 1_u8;
                server.send_to(&[ack], peer.unwrap()).await.unwrap();
            }
        });
    });

    server_addr
}

fn encode_udp_chunk(destination: &SocketAddr, payload: &[u8]) -> Vec<u8> {
    encode_udp_chunk_from_port(1234, destination, payload)
}

fn encode_udp_chunk_from_port(
    source_port: u16,
    destination: &SocketAddr,
    payload: &[u8],
) -> Vec<u8> {
    const APP_NAME: &str = "test";
    const SOURCE_IP: Ipv4Addr = Ipv4Addr::LOCALHOST;

    let mut buffer = vec![];
    buffer.put_u32((2 * (16 + 2) + 1 + APP_NAME.len() + payload.len()) as u32);
    buffer.put_slice(&[0; 12]);
    buffer.put_slice(&SOURCE_IP.octets());
    buffer.put_u16(source_port);
    buffer.put_slice(&[0; 12]);
    buffer.put_slice(&match destination.ip() {
        IpAddr::V4(ip) => ip.octets(),
        _ => unreachable!(),
    });
    buffer.put_u16(destination.port());
    buffer.put_u8(APP_NAME.len() as u8);
    buffer.put_slice(APP_NAME.as_bytes());
    buffer.put_slice(payload);

    buffer
}

async fn read_udp_reply(io: &mut (impl AsyncRead + Unpin)) -> Vec<u8> {
    let mut length = [0; 4];
    io.read_exact(&mut length).await.unwrap();
    let mut reply = vec![0; 4 + u32::from_be_bytes(length) as usize];
    reply[..4].copy_from_slice(&length);
    io.read_exact(&mut reply[4..]).await.unwrap();
    reply
}

fn reply_destination_port(reply: &[u8]) -> u16 {
    const DESTINATION_PORT_OFFSET: usize = 4 + (16 + 2) + 16;
    u16::from_be_bytes([
        reply[DESTINATION_PORT_OFFSET],
        reply[DESTINATION_PORT_OFFSET + 1],
    ])
}

async fn run_endpoint_with_socks5_forwarder(listen_address: &SocketAddr, socks: SocketAddr) {
    let settings = Settings::builder()
        .listen_address(listen_address)
        .unwrap()
        .listen_protocols(ListenProtocolSettings {
            http2: Some(Http2Settings::builder().build()),
            ..Default::default()
        })
        .allow_private_network_connections(true)
        .forwarder_settings(ForwardProtocolSettings::Socks5(
            Socks5ForwarderSettings::builder()
                .server_address(socks)
                .unwrap()
                .build()
                .unwrap(),
        ))
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

/// SOCKS5 server that accepts only UDP ASSOCIATE without authentication and echoes every
/// datagram back to the sender unchanged, as if the destination replied with the same payload.
fn run_socks5_udp_echo_server() -> SocketAddr {
    let listener = std::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    listener.set_nonblocking(true).unwrap();
    let address = listener.local_addr().unwrap();

    thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async move {
            let listener = TcpListener::from_std(listener).unwrap();
            loop {
                let (control, _) = listener.accept().await.unwrap();
                tokio::spawn(serve_socks5_udp_associate(control));
            }
        });
    });

    address
}

async fn serve_socks5_udp_associate(mut control: TcpStream) {
    let mut buf = [0; 64 * 1024];
    control.read_exact(&mut buf[..2]).await.unwrap();
    assert_eq!(buf[0], 0x05);
    let methods_num = buf[1] as usize;
    control.read_exact(&mut buf[..methods_num]).await.unwrap();
    assert!(
        buf[..methods_num].contains(&0x00),
        "{:?}",
        &buf[..methods_num]
    );
    control.write_all(&[0x05, 0x00]).await.unwrap();

    control.read_exact(&mut buf[..10]).await.unwrap();
    assert_eq!(
        &buf[..4],
        &[0x05, 0x03, 0x00, 0x01],
        "UDP ASSOCIATE expected"
    );

    let relay = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
    let mut reply = vec![0x05, 0x00, 0x00, 0x01];
    reply.extend_from_slice(&Ipv4Addr::LOCALHOST.octets());
    reply.extend_from_slice(&relay.local_addr().unwrap().port().to_be_bytes());
    control.write_all(&reply).await.unwrap();

    let mut probe = [0; 1];
    loop {
        tokio::select! {
            r = relay.recv_from(&mut buf) => {
                let (n, peer) = r.unwrap();
                relay.send_to(&buf[..n], peer).await.unwrap();
            }
            r = control.read(&mut probe) => match r {
                Ok(0) | Err(_) => break,
                Ok(_) => (),
            },
        }
    }
}
