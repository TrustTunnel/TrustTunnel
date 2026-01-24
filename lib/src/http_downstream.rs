use crate::downstream::Downstream;
use crate::http_codec::HttpCodec;
use crate::net_utils::TcpDestination;
use crate::tls_demultiplexer::Protocol;
use crate::{
    authentication, core, datagram_pipe, downstream, http_codec, http_datagram_codec,
    http_demultiplexer, http_forwarded_stream, http_icmp_codec, http_ping_handler,
    http_speedtest_handler, http_udp_codec, log_id, log_utils, net_utils, pipe, reverse_proxy,
    tunnel,
};
use async_trait::async_trait;
use bytes::Bytes;
use http::StatusCode;
use http_demultiplexer::HttpDemux;
use std::collections::LinkedList;
use std::io;
use std::io::ErrorKind;
use std::net::IpAddr;
use std::sync::Arc;

const HEALTH_CHECK_AUTHORITY: &str = "_check";
const UDP_AUTHORITY: &str = "_udp2";
const ICMP_AUTHORITY: &str = "_icmp";

const AUTHORIZATION_FAILURE_STATUS_CODE: StatusCode = StatusCode::PROXY_AUTHENTICATION_REQUIRED;
const AUTHORIZATION_FAILURE_EXTRA_HEADER: (&str, &str) =
    ("proxy-authenticate", "Basic realm=Authorization Required");

const BAD_STATUS_CODE: StatusCode = StatusCode::BAD_GATEWAY;
const WARNING_HEADER_NAME: &str = "X-Warning";
const DNS_WARNING_HEADER_NAME: &str = "X-Adguard-Vpn-Error";

pub(crate) struct HttpDownstream {
    context: Arc<core::Context>,
    codec: Box<dyn HttpCodec>,
    tls_domain: String,
    request_demux: HttpDemux,
}

struct TcpConnection {
    stream: Box<dyn http_codec::Stream>,
    id: log_utils::IdChain<u64>,
}

struct DatagramMultiplexer {
    stream: Box<dyn http_codec::Stream>,
    id: log_utils::IdChain<u64>,
}

struct DatagramEncoder<D> {
    encoder: Box<dyn http_datagram_codec::Encoder<Datagram = D>>,
    sink: Box<dyn pipe::Sink>,
}

struct DatagramDecoder<D> {
    source: Box<dyn pipe::Source>,
    decoder: Box<dyn http_datagram_codec::Decoder<Datagram = D>>,
    pending_bytes: LinkedList<Bytes>,
}

struct PendingRequest {
    stream: Box<dyn http_codec::Stream>,
    id: log_utils::IdChain<u64>,
}

impl HttpDownstream {
    pub fn new(context: Arc<core::Context>, codec: Box<dyn HttpCodec>, tls_domain: String) -> Self {
        Self {
            request_demux: HttpDemux::new(context.settings.clone()),
            context,
            codec,
            tls_domain,
        }
    }
}

#[async_trait]
impl Downstream for HttpDownstream {
    async fn listen(
        &mut self,
    ) -> io::Result<Option<Box<dyn downstream::PendingMultiplexedRequest>>> {
        loop {
            let stream = match self.codec.listen().await? {
                None => return Ok(None),
                Some(s) => s,
            };
            let request = stream.request().request();
            let stream_id = stream.id();
            log_id!(
                trace,
                stream_id,
                "HTTP downstream received request: {} {}",
                request.method,
                request.uri
            );
            log_id!(
                debug,
                stream_id,
                "Received request: {:?}",
                net_utils::scrub_request(request)
            );

            let protocol = self.protocol();
            let context = self.context.clone();
            let channel = self.request_demux.select(self.protocol(), request);
            log_id!(
                trace,
                stream_id,
                "HTTP downstream routing to channel: {:?}",
                channel
            );
            match channel {
                net_utils::Channel::Tunnel => {
                    log_id!(trace, stream_id, "HTTP downstream: tunnel request");
                    break Ok(Some(Box::new(PendingRequest {
                        stream,
                        id: stream_id,
                    })));
                }
                net_utils::Channel::Ping => {
                    log_id!(trace, stream_id, "HTTP downstream: ping request");
                    tokio::spawn(async move {
                        http_ping_handler::listen(
                            context.shutdown.clone(),
                            Box::new(http_codec::stream_into_codec(stream, protocol)),
                            context.settings.tls_handshake_timeout,
                            stream_id,
                        )
                        .await
                    });
                }
                net_utils::Channel::Speedtest => {
                    log_id!(trace, stream_id, "HTTP downstream: speedtest request");
                    tokio::spawn(async move {
                        http_speedtest_handler::listen(
                            context.shutdown.clone(),
                            Box::new(http_codec::stream_into_codec(stream, protocol)),
                            context.settings.tls_handshake_timeout,
                            stream_id,
                        )
                        .await
                    });
                }
                net_utils::Channel::ReverseProxy => {
                    log_id!(trace, stream_id, "HTTP downstream: reverse proxy request");
                    tokio::spawn({
                        let sni = self.tls_domain.clone();
                        async move {
                            reverse_proxy::listen(
                                context,
                                Box::new(http_codec::stream_into_codec(stream, protocol)),
                                sni,
                                stream_id,
                            )
                            .await
                        }
                    });
                }
            }
        }
    }

    async fn graceful_shutdown(&mut self) -> io::Result<()> {
        self.codec.graceful_shutdown().await
    }

    fn protocol(&self) -> Protocol {
        self.codec.protocol()
    }

    fn tls_domain(&self) -> &str {
        &self.tls_domain
    }
}

macro_rules! impl_stream_id {
    (for $($t:ty),+) => {
        $(impl downstream::StreamId for $t {
            fn id(&self) -> log_utils::IdChain<u64> {
                self.id.clone()
            }
        })*
    }
}

impl_stream_id!(for PendingRequest, TcpConnection, DatagramMultiplexer);

impl downstream::PendingRequest for TcpConnection {
    type NextState = (Box<dyn pipe::Source>, Box<dyn pipe::Sink>);

    fn promote_to_next_state(self: Box<Self>) -> io::Result<Self::NextState> {
        if self.stream.request().request().method == http::Method::CONNECT {
            let (source, sink) = self.stream.split();
            return Ok((
                source.finalize(),
                sink.send_ok_response(false)?.into_pipe_sink(),
            ));
        }

        http_forwarded_stream::into_forwarded(self.stream)
    }

    fn fail_request(self: Box<Self>, error: tunnel::ConnectionError) {
        fail_request_with_error(self.stream, error);
    }
}

impl downstream::PendingTcpConnectRequest for TcpConnection {
    fn client_address(&self) -> io::Result<IpAddr> {
        self.stream.request().client_address()
    }

    fn destination(&self) -> io::Result<TcpDestination> {
        let request = self.stream.request();
        let authority = request.authority()?;

        Ok(match authority.as_str().parse() {
            Ok(a) => TcpDestination::Address(a),
            Err(_) => {
                let port = if request.request().method == http::Method::CONNECT {
                    authority.port_u16().ok_or_else(|| {
                        io::Error::new(
                            ErrorKind::Other,
                            format!("Unexpected authority port: request={:?}", request.request()),
                        )
                    })?
                } else {
                    authority
                        .port_u16()
                        .unwrap_or(net_utils::PLAIN_HTTP_PORT_NUMBER)
                };

                TcpDestination::HostName((authority.host().to_string(), port))
            }
        })
    }

    fn user_agent(&self) -> Option<String> {
        self.stream.request().user_agent()
    }
}

impl downstream::PendingRequest for PendingRequest {
    type NextState = Option<downstream::PendingDemultiplexedRequest>;

    fn promote_to_next_state(self: Box<Self>) -> io::Result<Self::NextState> {
        let request = self.stream.request().request();

        match request.uri.authority().map(http::uri::Authority::as_str) {
            Some(HEALTH_CHECK_AUTHORITY) if request.method == http::Method::CONNECT => {
                self.stream.split().1.send_ok_response(true).map(|_| None)
            }
            Some(UDP_AUTHORITY) | Some(ICMP_AUTHORITY)
                if request.method == http::Method::CONNECT =>
            {
                Ok(Some(
                    downstream::PendingDemultiplexedRequest::DatagramMultiplexer(Box::new(
                        DatagramMultiplexer {
                            stream: self.stream,
                            id: self.id,
                        },
                    )),
                ))
            }
            Some(HEALTH_CHECK_AUTHORITY) | Some(UDP_AUTHORITY) | Some(ICMP_AUTHORITY) => {
                log_id!(debug, self.id, "Unexpected request method: {:?}", request);
                fail_request(self.stream, BAD_STATUS_CODE, vec![]);
                Ok(None)
            }
            _ => Ok(Some(downstream::PendingDemultiplexedRequest::TcpConnect(
                Box::new(TcpConnection {
                    stream: self.stream,
                    id: self.id,
                }),
            ))),
        }
    }

    fn fail_request(self: Box<Self>, error: tunnel::ConnectionError) {
        fail_request_with_error(self.stream, error);
    }
}

impl downstream::PendingMultiplexedRequest for PendingRequest {
    fn auth_info(&self) -> io::Result<Option<authentication::Source>> {
        self.stream.request().auth_info()
    }
}

impl downstream::PendingRequest for DatagramMultiplexer {
    type NextState = downstream::DatagramPipeHalves;

    fn promote_to_next_state(self: Box<Self>) -> io::Result<Self::NextState> {
        let authority = self.stream.request().authority()?.to_string();
        let (source, sink) = self.stream.split();
        match authority.as_str() {
            UDP_AUTHORITY => Ok(downstream::DatagramPipeHalves::Udp(
                Box::new(DatagramDecoder {
                    source: source.finalize(),
                    decoder: Box::new(http_udp_codec::Decoder::new(self.id.clone())),
                    pending_bytes: Default::default(),
                }),
                Box::new(DatagramEncoder {
                    sink: sink.send_ok_response(false)?.into_pipe_sink(),
                    encoder: Box::<http_udp_codec::Encoder>::default(),
                }),
            )),
            ICMP_AUTHORITY => Ok(downstream::DatagramPipeHalves::Icmp(
                Box::new(DatagramDecoder {
                    source: source.finalize(),
                    decoder: Box::new(http_icmp_codec::Decoder::new()),
                    pending_bytes: Default::default(),
                }),
                Box::new(DatagramEncoder {
                    sink: sink.send_ok_response(false)?.into_pipe_sink(),
                    encoder: Box::<http_icmp_codec::Encoder>::default(),
                }),
            )),
            _ => unreachable!(),
        }
    }

    fn fail_request(self: Box<Self>, error: tunnel::ConnectionError) {
        fail_request_with_error(self.stream, error);
    }
}

impl downstream::PendingDatagramMultiplexerRequest for DatagramMultiplexer {
    fn client_address(&self) -> io::Result<IpAddr> {
        self.stream.request().client_address()
    }

    fn user_agent(&self) -> Option<String> {
        self.stream.request().user_agent()
    }
}

impl<D> downstream::StreamId for DatagramDecoder<D> {
    fn id(&self) -> log_utils::IdChain<u64> {
        self.source.id()
    }
}

#[async_trait]
impl<D> datagram_pipe::Source for DatagramDecoder<D> {
    type Output = D;

    fn id(&self) -> log_utils::IdChain<u64> {
        self.source.id()
    }

    async fn read(&mut self) -> io::Result<D> {
        loop {
            let chunk = match self.pending_bytes.pop_front() {
                None => match self.source.read().await? {
                    pipe::Data::Chunk(bytes) => {
                        self.source.consume(bytes.len())?;
                        bytes
                    }
                    pipe::Data::Eof => return Err(io::Error::from(ErrorKind::UnexpectedEof)),
                },
                Some(bytes) => bytes,
            };

            match self.decoder.decode_chunk(chunk) {
                http_datagram_codec::DecodeResult::WantMore => (),
                http_datagram_codec::DecodeResult::Complete(datagram, tail) => {
                    if !tail.is_empty() {
                        self.pending_bytes.push_front(tail);
                    }

                    return Ok(datagram);
                }
            }
        }
    }
}

#[async_trait]
impl<D: Send> datagram_pipe::Sink for DatagramEncoder<D> {
    type Input = D;

    async fn write(&mut self, datagram: D) -> io::Result<datagram_pipe::SendStatus> {
        let bit_len = 0; // Don't know length yet
        
        let encoded = match self.encoder.encode_packet(&datagram) {
            None => {
                debug!("Failed to encode datagram");
                return Ok(datagram_pipe::SendStatus::Dropped);
            }
            Some(encoded) => encoded,
        };

        // write_all() ensures we wait until EVERYTHING is written.
        // This prevents partial writes which would corrupt the stream state (since we are tunneling datagrams).
        log::info!("HARDCORE UDP TX: len={}", encoded.len());
        self.sink.write_all(encoded).await?;
        self.sink.flush().await?;
        
        Ok(datagram_pipe::SendStatus::Sent)
        
    }
}

fn tunnel_error_to_status_code(error: &tunnel::ConnectionError) -> StatusCode {
    match error {
        tunnel::ConnectionError::Authentication(_) => AUTHORIZATION_FAILURE_STATUS_CODE,
        _ => BAD_STATUS_CODE,
    }
}

fn tunnel_error_to_warn_header(
    error: &tunnel::ConnectionError,
    hostname: &str,
) -> Vec<(String, String)> {
    match error {
        tunnel::ConnectionError::Io(_) => vec![(
            WARNING_HEADER_NAME.to_string(),
            "300 - Connection failed for some reason".to_string(),
        )],
        tunnel::ConnectionError::Authentication(_) => vec![(
            AUTHORIZATION_FAILURE_EXTRA_HEADER.0.to_string(),
            AUTHORIZATION_FAILURE_EXTRA_HEADER.1.to_string(),
        )],
        tunnel::ConnectionError::Timeout => {
            vec![(WARNING_HEADER_NAME.to_string(), format!("302 - {}", error))]
        }
        tunnel::ConnectionError::HostUnreachable => {
            vec![(WARNING_HEADER_NAME.to_string(), format!("301 - {}", error))]
        }
        tunnel::ConnectionError::DnsNonroutable => vec![
            (DNS_WARNING_HEADER_NAME.to_string(), hostname.to_string()),
            (WARNING_HEADER_NAME.to_string(), format!("310 - {}", error)),
        ],
        tunnel::ConnectionError::DnsLoopback => vec![
            (DNS_WARNING_HEADER_NAME.to_string(), hostname.to_string()),
            (WARNING_HEADER_NAME.to_string(), format!("311 - {}", error)),
        ],
        tunnel::ConnectionError::Other(_) => vec![(
            WARNING_HEADER_NAME.to_string(),
            "300 - Connection failed for some reason".to_string(),
        )],
    }
}

fn fail_request(
    stream: Box<dyn http_codec::Stream>,
    status: StatusCode,
    extra_headers: Vec<(String, String)>,
) {
    let id = stream.id();
    if let Err(e) = stream.split().1.send_bad_response(status, extra_headers) {
        log_id!(debug, id, "Failed to send bad response: {}", e);
    }
}

fn fail_request_with_error(stream: Box<dyn http_codec::Stream>, error: tunnel::ConnectionError) {
    let extra_headers = tunnel_error_to_warn_header(&error, request_hostname(stream.request()));
    fail_request(stream, tunnel_error_to_status_code(&error), extra_headers);
}

fn request_hostname(request: &dyn http_codec::PendingRequest) -> &str {
    request
        .authority()
        .map(http::uri::Authority::as_str)
        .unwrap_or_default()
}
