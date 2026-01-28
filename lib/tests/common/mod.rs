use bytes::{Buf, Bytes, BytesMut};
use futures::future;
use http::{Request, Response};
use hyper::body::HttpBody;
use log::{info, LevelFilter};
use quiche::h3;
use quiche::h3::NameValue;
use ring::rand::{SecureRandom, SystemRandom};
use rustls::client::ServerCertVerified;
use rustls::{Certificate, ServerName};
use std::io::{ErrorKind, Write};
use std::net::{Ipv4Addr, SocketAddr};
use std::ops::Deref;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::atomic::{AtomicU16, Ordering};
use std::sync::{Arc, Once};
use std::time::{Duration, SystemTime};
use std::{iter, slice};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::{TcpStream, UdpSocket};
use tokio_rustls::TlsConnector;
use trusttunnel::authentication::{registry_based::RegistryBasedAuthenticator, Authenticator};
use trusttunnel::core::Core;
use trusttunnel::log_utils;
use trusttunnel::settings::{
    Http1Settings, Http2Settings, ListenProtocolSettings, QuicSettings, Settings, TlsHostInfo,
    TlsHostsSettings,
};
use trusttunnel::shutdown::Shutdown;

pub const MAIN_DOMAIN_NAME: &str = "localhost";
pub const ENDPOINT_IP: Ipv4Addr = Ipv4Addr::LOCALHOST;
pub static NEXT_ENDPOINT_PORT: AtomicU16 = AtomicU16::new(9128);

pub fn set_up_logger() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        log::set_max_level(LevelFilter::Debug);
        log::set_logger(log_utils::make_stdout_logger()).unwrap();
    });
}

pub fn make_endpoint_address() -> SocketAddr {
    (
        ENDPOINT_IP,
        NEXT_ENDPOINT_PORT.fetch_add(1, Ordering::Relaxed),
    )
        .into()
}

pub fn make_cert_key_file() -> File {
    let file = File::new(std::env::temp_dir().join(format!("vle-{}.pem",
                          trusttunnel::utils::hex_dump(
                              ring::rand::generate::<[u8; 16]>(&SystemRandom::new())
                                  .unwrap().expose().as_slice()
                          )
            )));

    std::fs::File::create(&file.path)
        .unwrap()
        .write_all(CERT_KEY.as_bytes())
        .unwrap();

    file
}

pub async fn establish_tls_connection(
    server_name: &str,
    peer: &SocketAddr,
    alpn: Option<&[u8]>,
) -> impl AsyncRead + AsyncWrite + Unpin {
    let mut config = rustls::ClientConfig::builder()
        .with_safe_defaults()
        .with_custom_certificate_verifier(Arc::new(NoopVerifier {}))
        .with_no_client_auth();
    if let Some(alpn) = alpn {
        config.alpn_protocols.push(alpn.to_vec());
    }

    TlsConnector::from(Arc::new(config))
        .connect(
            ServerName::try_from(server_name).unwrap(),
            TcpStream::connect(peer).await.unwrap(),
        )
        .await
        .unwrap()
}

pub fn make_stream_of_chunks(
    total_size: usize,
    chunk_size: Option<usize>,
) -> futures::stream::Iter<impl Iterator<Item = &'static [u8]>> {
    const SIZE: usize = 16 * 1024;

    let size = chunk_size.unwrap_or(SIZE);
    assert!(total_size >= size, "{total_size}");
    assert_eq!(total_size % size, 0, "{total_size}");

    static CHUNK: [u8; SIZE] = [0; SIZE];

    futures::stream::iter(iter::repeat(&CHUNK[..size]).take(total_size / size))
}

pub struct File {
    pub path: PathBuf,
}

impl File {
    fn new(path: PathBuf) -> Self {
        Self { path }
    }
}

impl Drop for File {
    fn drop(&mut self) {
        std::fs::remove_file(&self.path).unwrap();
    }
}

pub struct NoopVerifier;

impl rustls::client::ServerCertVerifier for NoopVerifier {
    fn verify_server_cert(
        &self,
        _: &Certificate,
        _: &[Certificate],
        _: &ServerName,
        _: &mut dyn Iterator<Item = &[u8]>,
        _: &[u8],
        _: SystemTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        Ok(ServerCertVerified::assertion())
    }
}

/// CN = [`MAIN_DOMAIN_NAME`]
const CERT_KEY: &str = "
-----BEGIN CERTIFICATE-----
MIIEYzCCA0ugAwIBAgIJAPoYqB3toabPMA0GCSqGSIb3DQEBCwUAMIGOMQswCQYD
VQQGEwJNQzERMA8GA1UECAwITXkgU3RhdGUxFDASBgNVBAcMC015IExvY2FsaXR5
MSAwHgYDVQQKDBdNeSBPcmdhbml6YXRpb24gTGltaXRlZDESMBAGA1UEAwwJbG9j
YWxob3N0MSAwHgYJKoZIhvcNAQkBFhFzdXBwb3J0QGVtYWlsLmNvbTAeFw0yMzAz
MDMxMzQ0MDVaFw0yNTExMjcxMzQ0MDVaMIGOMQswCQYDVQQGEwJNQzERMA8GA1UE
CAwITXkgU3RhdGUxFDASBgNVBAcMC015IExvY2FsaXR5MSAwHgYDVQQKDBdNeSBP
cmdhbml6YXRpb24gTGltaXRlZDESMBAGA1UEAwwJbG9jYWxob3N0MSAwHgYJKoZI
hvcNAQkBFhFzdXBwb3J0QGVtYWlsLmNvbTCCASIwDQYJKoZIhvcNAQEBBQADggEP
ADCCAQoCggEBAN109RwtqlimLcptek+vtoulGtQi7XQ8H846gpMYdNXMSmdkk/vN
Gf3t+43GEehryzQLGINZgyNmWZX+j8K3lvPuXKvbRUKa3tISj2h73+DEwfzR4/Lg
szrKdlDRi/ej9H8mo/9kdTMrK2s2Zzg4JBQmAFepR57jKVoNsj4bRL6pv1+yQcdP
U0GjS6yp+ebAeJpI8n6cNndKG+yovpAHLgwvRyF91Ds+OPco5hznSQrU71qHb0fD
XkLrlOeLrgMGrIv7Rb8APRAC2dmAkj3dNeYlggOcc1Gy2tR7eXt1maFCF7ebsxNU
WNN1lbTzLShTfv3wqghajjKpVU9/m7lQ/2sCAwEAAaOBwTCBvjAdBgNVHQ4EFgQU
zz3RamEP0LRqB/+mqrYWiSyilogwHwYDVR0jBBgwFoAUzz3RamEP0LRqB/+mqrYW
iSyilogwCQYDVR0TBAIwADALBgNVHQ8EBAMCBaAwNgYDVR0RBC8wLYIJbG9jYWxo
b3N0ghVsb2NhbGhvc3QubG9jYWxkb21haW6CCTEyNy4wLjAuMTAsBglghkgBhvhC
AQ0EHxYdT3BlblNTTCBHZW5lcmF0ZWQgQ2VydGlmaWNhdGUwDQYJKoZIhvcNAQEL
BQADggEBAFvQdL2bMg5OL83B6QqGlPN9qjGl/PjTlyeIliekSQpfbQe+Q0Sqq8Qc
+a8T0dxiIVIPmfhwZ3rxb6OCWAnGf1HN3Mfm8eTd2Vjn/PgoTb6n7uZVr8P2pbfO
X5mmFdG1V34sMh52GB1mhqEDxuLEDD6Y6NJaMn6TyUBcKtgU8UZGJPUy8mD3EB3u
IVt+sB6OIia5xPpDI+lZkFjY3HuqfMX6lEgV7mdkUJetkqtwLAqyDcut3oH4TVKh
dMbkIyCElsl8NJpRZSbvoCKCKRhuaxlHW4Rf5HuLcKHL0wvk/cwZa4dD9qKSLyBc
vOUVSnFoxGwBMhsbDovY1UExeGYuNTs=
-----END CERTIFICATE-----
-----BEGIN PRIVATE KEY-----
MIIEvwIBADANBgkqhkiG9w0BAQEFAASCBKkwggSlAgEAAoIBAQDddPUcLapYpi3K
bXpPr7aLpRrUIu10PB/OOoKTGHTVzEpnZJP7zRn97fuNxhHoa8s0CxiDWYMjZlmV
/o/Ct5bz7lyr20VCmt7SEo9oe9/gxMH80ePy4LM6ynZQ0Yv3o/R/JqP/ZHUzKytr
Nmc4OCQUJgBXqUee4ylaDbI+G0S+qb9fskHHT1NBo0usqfnmwHiaSPJ+nDZ3Shvs
qL6QBy4ML0chfdQ7Pjj3KOYc50kK1O9ah29Hw15C65Tni64DBqyL+0W/AD0QAtnZ
gJI93TXmJYIDnHNRstrUe3l7dZmhQhe3m7MTVFjTdZW08y0oU3798KoIWo4yqVVP
f5u5UP9rAgMBAAECggEBALlHtBbaQe4fQqpdA/sNiM222gZoHoCkGPwiycIlsQJ7
BDkS1hjSlY90/4SzFaJ+JSmqqtyiFGyWohczPrXrgfkeERybvIuoJQpfCuqg0UMt
ext5w5wd0PY8E9c0KkWLP/DttEHlm4Su9omhn6RSnCTbUmgFMe3GIn+8e8coa1CU
CA+e2yc5XrC2Y/yiPVsyDwwvoitXLk27Cnyva04dvJKPa/ZeQWe7GQ3PD4SYzx4s
+tuy3+2MuHvKx/LkPKVBJfk7cNTtJKBmZfwlq1stK+RA+DNolhzX8d2FmMyNRDvu
OOaxBgfHhSXdtKIz8c9wCxJg1YslQ30OeiAbJ4S5IaECgYEA8v0K7nQ048pULDfa
vR3Cxkd+KOYMYFnuxVn3OaeOI2VJ6h4gboJ8Ay/vtvHhv9ir7AuvQ/Ceuexe5B4Q
GTfeMH2IoaRQeWgsjaYBFYbgSirpUMhcCeVhVf8HXyMg2MFE+WTJIchWZ19i0OAl
CYnXy+mB1IeQFbqGdF6bQoW4DPECgYEA6VDA44N9PSiKMfHqhJAIg2UuAlUapOoQ
D4S4SgMfZnzWrpDO0d4IYAvPEXKOjiK9B9fNjJ/GKE1KOISWc+5/eW0TMdAPI0gE
bxDe1Tp2JMO7sDNAB/xrOPUccpiCZJC8oeva6rUyhRiRgh4u+f+wsZkKDAf6xG4/
aM/2AzqpwhsCgYBEmz2i5hyo1E+/zGVuUCDWawkr8wg7jCjmf+hV1wFC7S5Zc/gk
O6NYIwjD1reuuzaPhx0NSbsHM733GqXg+O07M7aILSSrosYxmFVmBpb9WfBWZrvV
73X0GfWy3vA/QxJ+d/5yE2aR+VSlNSQ/9TOA14VYxI3iFLAx2yRrO+YjgQKBgQDW
belZMFfCBag9DuFCxD2OxUbrzduXBaeNG6VkIEqTntiPx3bNWwrHexLsLiTmbPbe
Zm/7djxgfehg2TqNgfyWVLD3bwj6nA23JgImZnx+fYXaAsAulsbUqjFjANeWJY+4
IVQpsi6kNFhHBgaWrXBvSP/63rqSHeEZK0gm35t1UQKBgQC/gmaQpb3w8UvQZG4p
8vrvqrZxvF0OOvnggsgpP71191naiEO3+pby/efFgutqJdXWJXuyWeg1W7loMejL
tBkmxjMw8cFLCP9o7W7QSb9XIqfCyg4dX4Fl9l1fDNX/xK2c3dlDJv6Spi1IMdFY
0GPe2vRXo0vDDFbEyL6MqgsH0w==
-----END PRIVATE KEY-----
";

pub async fn run_endpoint(listen_address: &SocketAddr) {
    let settings = Settings::builder()
        .listen_address(listen_address)
        .unwrap()
        .listen_protocols(ListenProtocolSettings {
            http1: Some(Http1Settings::builder().build()),
            http2: Some(Http2Settings::builder().build()),
            quic: Some(QuicSettings::builder().build()),
        })
        .allow_private_network_connections(true)
        .speedtest_enable(true)
        .ping_enable(true)
        .ping_path("/ping")
        .speedtest_path("/speed")
        .build()
        .unwrap();

    let cert_key_file = make_cert_key_file();
    let cert_key_path = cert_key_file.path.to_str().unwrap();
    let hosts_settings = TlsHostsSettings::builder()
        .main_hosts(vec![TlsHostInfo {
            hostname: MAIN_DOMAIN_NAME.to_string(),
            cert_chain_path: cert_key_path.to_string(),
            private_key_path: cert_key_path.to_string(),
            allowed_sni: vec![],
        }])
        .ping_hosts(vec![TlsHostInfo {
            hostname: format!("ping.{}", MAIN_DOMAIN_NAME),
            cert_chain_path: cert_key_path.to_string(),
            private_key_path: cert_key_path.to_string(),
            allowed_sni: vec![],
        }])
        .speedtest_hosts(vec![TlsHostInfo {
            hostname: format!("speed.{}", MAIN_DOMAIN_NAME),
            cert_chain_path: cert_key_path.to_string(),
            private_key_path: cert_key_path.to_string(),
            allowed_sni: vec![],
        }])
        .reverse_proxy_hosts(vec![TlsHostInfo {
            hostname: format!("hello.{}", MAIN_DOMAIN_NAME),
            cert_chain_path: cert_key_path.to_string(),
            private_key_path: cert_key_path.to_string(),
            allowed_sni: vec![],
        }])
        .build()
        .unwrap();

    run_endpoint_with_settings(settings, hosts_settings).await;
}

pub async fn run_endpoint_with_settings(settings: Settings, hosts_settings: TlsHostsSettings) {
    let shutdown = Shutdown::new();
    let authenticator: Option<Arc<dyn Authenticator>> = if !settings.get_clients().is_empty() {
        Some(Arc::new(RegistryBasedAuthenticator::new(
            settings.get_clients(),
        )))
    } else {
        None
    };

    let endpoint = Core::new(settings, authenticator, hosts_settings, shutdown).unwrap();
    endpoint.listen().await.unwrap();
}

const MAX_QUIC_UDP_PAYLOAD_SIZE: usize = 1350;

pub struct Http3Session {
    socket: UdpSocket,
    quic_conn: quiche::Connection,
    h3_conn: h3::Connection,
    stream_id: Option<u64>,
    is_tunnel: bool, // True for CONNECT requests - don't send FIN
}

impl Http3Session {
    pub async fn connect(peer: &SocketAddr, server_name: &str, alpn: Option<&[u8]>) -> Self {
        let socket = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();

        let mut scid = [0; quiche::MAX_CONN_ID_LEN];
        SystemRandom::new().fill(&mut scid[..]).unwrap();

        let mut config = quiche::Config::new(quiche::PROTOCOL_VERSION).unwrap();
        config.verify_peer(false);
        config.set_max_idle_timeout(5000);
        config.set_max_recv_udp_payload_size(MAX_QUIC_UDP_PAYLOAD_SIZE);
        config.set_max_send_udp_payload_size(MAX_QUIC_UDP_PAYLOAD_SIZE);
        config.set_initial_max_data(10_000_000);
        config.set_initial_max_stream_data_bidi_local(1_000_000);
        config.set_initial_max_stream_data_bidi_remote(1_000_000);
        config.set_initial_max_stream_data_uni(1_000_000);
        config.set_initial_max_streams_bidi(100);
        config.set_initial_max_streams_uni(100);
        config
            .set_application_protos(
                alpn.as_ref()
                    .map_or(h3::APPLICATION_PROTOCOL, slice::from_ref),
            )
            .unwrap();

        let mut quic_conn = quiche::connect(
            Some(server_name),
            &quiche::ConnectionId::from_ref(&scid),
            socket.local_addr().unwrap(),
            *peer,
            &mut config,
        )
        .unwrap();

        // avoid would block
        tokio::time::sleep(Duration::from_millis(100)).await;
        Self::flush_quic_data(&socket, &mut quic_conn);

        while !quic_conn.is_established() {
            let _ = tokio::time::timeout(quic_conn.timeout().unwrap(), socket.readable()).await;

            Self::read_out_socket(&socket, &mut quic_conn);
            Self::flush_quic_data(&socket, &mut quic_conn);

            if quic_conn.is_closed() {
                panic!("Closed");
            }
        }

        let h3_conn =
            h3::Connection::with_transport(&mut quic_conn, &h3::Config::new().unwrap()).unwrap();
        Self::flush_quic_data(&socket, &mut quic_conn);

        Self {
            socket,
            quic_conn,
            h3_conn,
            stream_id: Default::default(),
            is_tunnel: false,
        }
    }

    pub async fn exchange(
        &mut self,
        request: Request<hyper::Body>,
    ) -> (http::response::Parts, Bytes) {
        let method = request.method().clone();
        self.send_request(request).await;
        let response = self.recv_response_with_method(&method).await;

        let content_length = (method == http::Method::CONNECT).then_some(0).or_else(|| {
            response
                .headers
                .get(http::header::CONTENT_LENGTH)
                .map(|x| x.to_str().unwrap().parse::<usize>().unwrap())
        });
        let mut content = BytesMut::with_capacity(content_length.unwrap_or_default());
        while content_length.is_none_or(|x| content.len() < x) {
            let mut buffer = [0; 64 * 1024];
            match self.recv(&mut buffer).await {
                0 => break,
                n => content.extend_from_slice(&buffer[..n]),
            }
        }

        (response, content.freeze())
    }

    pub async fn send_request(&mut self, mut request: Request<hyper::Body>) {
        let uri = request.uri();
        let req = iter::once(h3::Header::new(
            b":method",
            request.method().as_str().as_bytes(),
        ))
        .chain(match uri.scheme_str() {
            Some(x) => Box::new(iter::once(h3::Header::new(b":scheme", x.as_bytes())))
                as Box<dyn Iterator<Item = h3::Header>>,
            None => Box::new(iter::empty()) as Box<dyn Iterator<Item = h3::Header>>,
        })
        .chain(iter::once(h3::Header::new(
            b":authority",
            uri.authority().unwrap().as_str().as_bytes(),
        )))
        .chain(match uri.path_and_query() {
            Some(x) => Box::new(iter::once(h3::Header::new(b":path", x.as_str().as_bytes())))
                as Box<dyn Iterator<Item = h3::Header>>,
            None => Box::new(iter::empty()) as Box<dyn Iterator<Item = h3::Header>>,
        })
        .chain(
            request
                .headers()
                .iter()
                .map(|(n, v)| h3::Header::new(n.as_str().as_bytes(), v.as_bytes())),
        )
        .collect::<Vec<_>>();

        // Always send request with fin=false, we'll send FIN separately after body
        self.stream_id = Some(
            self.h3_conn
                .send_request(&mut self.quic_conn, &req, false)
                .unwrap(),
        );
        Self::flush_quic_data(&self.socket, &mut self.quic_conn);

        while let Some(mut chunk) = request.body_mut().data().await.map(Result::unwrap) {
            while !chunk.is_empty() {
                let stream_id = self.stream_id();
                match self
                    .h3_conn
                    .send_body(&mut self.quic_conn, stream_id, &chunk, false)
                {
                    Ok(n) => chunk.advance(n),
                    Err(h3::Error::Done) => {
                        Self::flush_quic_data(&self.socket, &mut self.quic_conn);
                        let _ = tokio::time::timeout(
                            self.quic_conn.timeout().unwrap(),
                            self.socket.readable(),
                        )
                        .await;
                    }
                    Err(e) => panic!("{}", e),
                }

                Self::read_out_socket(&self.socket, &mut self.quic_conn);
                Self::flush_quic_data(&self.socket, &mut self.quic_conn);
            }
        }

        Self::flush_quic_data(&self.socket, &mut self.quic_conn);
    }

    pub async fn recv_response(&mut self) -> http::response::Parts {
        self.recv_response_with_method(&http::Method::GET).await
    }

    async fn recv_response_with_method(&mut self, method: &http::Method) -> http::response::Parts {
        Self::read_out_socket(&self.socket, &mut self.quic_conn);
        Self::flush_quic_data(&self.socket, &mut self.quic_conn);

        loop {
            match self.poll().await {
                h3::Event::Headers { list, .. } => {
                    let mut response = Response::builder().version(http::Version::HTTP_3);
                    for h in list {
                        match h.name() {
                            b":status" => response = response.status(h.value()),
                            _ => response = response.header(h.name(), h.value()),
                        }
                    }

                    let response = response.body(()).unwrap().into_parts().0;
                    info!("Received response: {:?}", response);

                    // Track if this is a CONNECT tunnel - don't send FIN for tunnels
                    if method == http::Method::CONNECT {
                        self.is_tunnel = true;
                    }

                    if !self.is_tunnel {
                        loop {
                            let stream_id = self.stream_id();
                            match self
                                .h3_conn
                                .send_body(&mut self.quic_conn, stream_id, &[], true)
                            {
                                Ok(_) => break,
                                Err(h3::Error::Done) => {
                                    Self::flush_quic_data(&self.socket, &mut self.quic_conn);
                                    let _ = tokio::time::timeout(
                                        self.quic_conn.timeout().unwrap(),
                                        self.socket.readable(),
                                    )
                                    .await;
                                    Self::read_out_socket(&self.socket, &mut self.quic_conn);
                                }
                                // If stream/connection already closed, that's fine
                                Err(
                                    h3::Error::TransportError(_)
                                    | h3::Error::StreamBlocked
                                    | h3::Error::IdError,
                                ) => {
                                    break;
                                }
                                Err(e) => panic!("Failed to finish stream: {}", e),
                            }
                        }
                        Self::flush_quic_data(&self.socket, &mut self.quic_conn);
                    }

                    return response;
                }
                h3::Event::Finished => {
                    // Client-side FIN received (normal for requests without body)
                    // Continue polling for response headers
                    continue;
                }
                x => unreachable!("{:?}", x),
            }
        }
    }

    fn read_out_socket(socket: &UdpSocket, quic_conn: &mut quiche::Connection) {
        let mut buffer = [0; MAX_QUIC_UDP_PAYLOAD_SIZE];
        loop {
            match socket.try_recv_from(&mut buffer) {
                Ok((n, peer)) => {
                    let recv_info = quiche::RecvInfo {
                        from: peer,
                        to: socket.local_addr().unwrap(),
                    };
                    let x = quic_conn.recv(&mut buffer[..n], recv_info).unwrap();
                    assert_eq!(n, x);
                }
                Err(e) if e.kind() == ErrorKind::WouldBlock => break,
                Err(e) => panic!("{}", e),
            }
        }
    }

    pub async fn send(
        &mut self,
        mut stream: impl futures::stream::Stream<Item = impl Deref<Target = [u8]>> + Unpin,
    ) {
        while let Some(mut chunk) =
            futures::future::poll_fn(|cx| Pin::new(&mut stream).poll_next(cx))
                .await
                .as_deref()
        {
            while !chunk.is_empty() {
                let stream_id = self.stream_id();
                match self
                    .h3_conn
                    .send_body(&mut self.quic_conn, stream_id, chunk, false)
                {
                    Ok(n) => chunk = &chunk[n..],
                    Err(h3::Error::Done) => {
                        Self::flush_quic_data(&self.socket, &mut self.quic_conn);
                        let _ = tokio::time::timeout(
                            self.quic_conn.timeout().unwrap(),
                            self.socket.readable(),
                        )
                        .await;
                    }
                    Err(e) => panic!("{}", e),
                }

                Self::read_out_socket(&self.socket, &mut self.quic_conn);
                Self::flush_quic_data(&self.socket, &mut self.quic_conn);
            }
        }

        // Don't send FIN for CONNECT tunnels - they remain bidirectional
        if !self.is_tunnel {
            loop {
                let stream_id = self.stream_id();
                match self
                    .h3_conn
                    .send_body(&mut self.quic_conn, stream_id, &[], true)
                {
                    Ok(_) => break,
                    Err(h3::Error::Done) => {
                        Self::flush_quic_data(&self.socket, &mut self.quic_conn);
                        let _ = tokio::time::timeout(
                            self.quic_conn.timeout().unwrap(),
                            self.socket.readable(),
                        )
                        .await;
                        Self::read_out_socket(&self.socket, &mut self.quic_conn);
                    }
                    // If stream/connection already closed
                    Err(
                        h3::Error::TransportError(_)
                        | h3::Error::StreamBlocked
                        | h3::Error::IdError,
                    ) => {
                        break;
                    }
                    Err(e) => panic!("Failed to finish stream: {}", e),
                }
            }
        }

        Self::flush_quic_data(&self.socket, &mut self.quic_conn);
    }

    pub async fn recv(&mut self, buf: &mut [u8]) -> usize {
        let ret = loop {
            Self::read_out_socket(&self.socket, &mut self.quic_conn);

            let stream_id = self.stream_id();
            match self.h3_conn.recv_body(&mut self.quic_conn, stream_id, buf) {
                Ok(n) => break n,
                Err(h3::Error::Done) => (),
                Err(e) => panic!("{}", e),
            }

            Self::flush_quic_data(&self.socket, &mut self.quic_conn);

            let _ = tokio::time::timeout(self.quic_conn.timeout().unwrap(), self.socket.readable())
                .await;

            Self::read_out_socket(&self.socket, &mut self.quic_conn);

            match self.poll().await {
                h3::Event::Data => (),
                h3::Event::Finished | h3::Event::Reset(_) => break 0,
                x => unreachable!("{:?}", x),
            }
        };

        Self::flush_quic_data(&self.socket, &mut self.quic_conn);

        ret
    }

    fn flush_quic_data(socket: &UdpSocket, quic_conn: &mut quiche::Connection) {
        let mut buffer = [0; MAX_QUIC_UDP_PAYLOAD_SIZE];
        loop {
            match quic_conn.send(&mut buffer) {
                Ok((n, send_info)) => match socket.try_send_to(&buffer[..n], send_info.to) {
                    Ok(_) => (),
                    Err(e) if e.kind() == ErrorKind::WouldBlock => break,
                    Err(e) => panic!("{}", e),
                },
                Err(quiche::Error::Done) => break,
                Err(e) => panic!("{}", e),
            }
        }
    }

    async fn poll(&mut self) -> h3::Event {
        Self::read_out_socket(&self.socket, &mut self.quic_conn);

        let ret = loop {
            match self.h3_conn.poll(&mut self.quic_conn) {
                Ok((stream_id, event)) => {
                    assert_eq!(stream_id, self.stream_id.unwrap());
                    break event;
                }
                Err(h3::Error::Done) => (),
                Err(e) => panic!("{}", e),
            }

            Self::flush_quic_data(&self.socket, &mut self.quic_conn);
            let _ = tokio::time::timeout(self.quic_conn.timeout().unwrap(), self.socket.readable())
                .await;
            Self::read_out_socket(&self.socket, &mut self.quic_conn);

            if self.quic_conn.is_closed() {
                panic!("Closed");
            }
        };

        Self::flush_quic_data(&self.socket, &mut self.quic_conn);

        ret
    }

    fn stream_id(&self) -> u64 {
        self.stream_id.unwrap()
    }
}

pub async fn do_get_request<IO>(
    io: IO,
    version: http::Version,
    url: &str,
    extra_headers: &[(&str, &str)],
) -> (http::response::Parts, Bytes)
where
    IO: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let (mut request, conn) = hyper::client::conn::Builder::new()
        .http2_only(version == http::Version::HTTP_2)
        .handshake(io)
        .await
        .unwrap();

    let mut request_builder = hyper::Request::get(url).version(version);
    for (n, v) in extra_headers {
        request_builder = request_builder.header(*n, *v);
    }

    let exchange = async {
        let response = request
            .send_request(request_builder.body(hyper::Body::empty()).unwrap())
            .await
            .unwrap();
        info!("Received response: {:?}", response);

        let (parts, body) = response.into_parts();
        (parts, hyper::body::to_bytes(body).await.unwrap())
    };

    futures::pin_mut!(exchange);
    match future::select(conn, exchange).await {
        future::Either::Left((r, exchange)) => {
            info!("HTTP connection closed with result: {:?}", r);
            exchange.await
        }
        future::Either::Right((response, _)) => response,
    }
}

pub async fn do_post_request<IO>(
    io: IO,
    version: http::Version,
    url: &str,
    content_length: usize,
) -> Response<hyper::Body>
where
    IO: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let (mut request, conn) = hyper::client::conn::Builder::new()
        .http2_only(version == http::Version::HTTP_2)
        .handshake(io)
        .await
        .unwrap();

    let exchange = async {
        let req = hyper::Request::post(url)
            .version(version)
            .body(hyper::Body::from(vec![0; content_length]))
            .unwrap();

        let response = request.send_request(req).await.unwrap();

        info!("Received response: {:?}", response);
        response
    };

    futures::pin_mut!(exchange);
    match future::select(conn, exchange).await {
        future::Either::Left((r, exchange)) => {
            info!("HTTP connection closed with result: {:?}", r);
            exchange.await
        }
        future::Either::Right((response, _)) => response,
    }
}
