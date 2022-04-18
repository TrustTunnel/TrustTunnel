use std::collections::LinkedList;
use std::io;
use std::io::ErrorKind;
use std::sync::Arc;
use http::StatusCode;
use async_trait::async_trait;
use bytes::Bytes;
use crate::downstream::Downstream;
use crate::{authorization, datagram_pipe, downstream, http_codec, http_datagram_codec, http_forwarded_stream, http_icmp_codec, http_udp_codec, log_id, log_utils, net_utils, pipe};
use crate::http_codec::HttpCodec;
use crate::net_utils::TcpDestination;
use crate::settings::Settings;


const HEALTH_CHECK_AUTHORITY: &str = "_check";
const UDP_AUTHORITY: &str = "_udp2";
const ICMP_AUTHORITY: &str = "_icmp";

const AUTHORIZATION_FAILURE_STATUS_CODE: StatusCode = StatusCode::PROXY_AUTHENTICATION_REQUIRED;
const AUTHORIZATION_FAILURE_EXTRA_HEADER: (&str, &str) =
    ("proxy-authenticate", "Basic realm=Authorization Required");


const BAD_STATUS_CODE: StatusCode = StatusCode::BAD_GATEWAY;
const WARNING_HEADER_NAME: &str = "X-Warning";

/// DNS resolution failed (reasons see below)
/// HTTP/1.1 502 Bad Gateway
/// X-Adguard-Vpn-Error: <hostname>
/// X-Warning: <warn-code> - <warn-text>
///
/// For other reasons:
/// HTTP/1.1 502 Bad Gateway
/// X-Warning: <warn-code> - <warn-text>
struct Warning {
    code: u32,
    text: &'static str,
}

impl Warning {
    const fn new(code: u32, text: &'static str) -> Self {
        Warning{ code, text }
    }

    fn make_header_value(&self) -> String {
        format!("{} - {}", self.code, self.text)
    }

    const CONNECTION_FAILED: Warning = Warning::new(300, "Connection failed for some reasons");
    const HOST_UNREACHABLE: Warning = Warning::new(301, "Remote host is unreachable");
    const CONNECTION_TIMEDOUT: Warning = Warning::new(302, "Connection timed out");
    const DNS_NONROUTABLE: Warning = Warning::new(310, "DNS: resolved address in non-routable network");
    const DNS_LOOPBACK: Warning = Warning::new(311, "DNS: resolved address in loopback");
    const DNS_BLOCKED: Warning = Warning::new(312, "DNS: blocked by Adguard DNS");
}


pub(crate) struct HttpDownstream {
    codec: Box<dyn HttpCodec>,
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
    sink: Box<dyn http_codec::DroppingSink>,
}

struct DatagramDecoder<D> {
    source: Box<dyn pipe::Source>,
    decoder: Box<dyn http_datagram_codec::Decoder<Datagram = D>>,
    pending_bytes: LinkedList<Bytes>,
}

struct PendingAuthorization {
    stream: Box<dyn http_codec::Stream>,
    id: log_utils::IdChain<u64>,
}

impl HttpDownstream {
    pub fn new(
        _core_settings: Arc<Settings>,
        codec: Box<dyn HttpCodec>,
    ) -> Self {
        Self {
            codec,
        }
    }
}

#[async_trait]
impl Downstream for HttpDownstream {
    async fn listen(&mut self) -> io::Result<Option<Box<dyn downstream::AuthorizationRequest>>> {
        loop {
            let stream = match self.codec.listen().await? {
                None => return Ok(None),
                Some(s) => s,
            };
            let request = stream.request().request();
            let stream_id = stream.id();
            log_id!(trace, stream_id, "Received request: {:?}", request);

            let authority = match request.uri.authority() {
                Some(a) => a.to_string(),
                None => {
                    log_id!(debug, stream_id, "Authority not found: {:?}", request);
                    fail_request(stream, BAD_STATUS_CODE, vec![]);
                    continue;
                }
            };

            let (is_health_check, is_datagram_mux) = match authority.as_str() {
                HEALTH_CHECK_AUTHORITY => (true, false),
                UDP_AUTHORITY | ICMP_AUTHORITY => (false, true),
                _ => (false, false),
            };

            if (is_health_check || is_datagram_mux) && request.method != http::Method::CONNECT {
                log_id!(debug, stream_id, "Unexpected request method: {:?}", request);
                fail_request(stream, BAD_STATUS_CODE, vec![]);
                continue;
            }

            if is_health_check {
                break Ok(Some(Box::new(PendingAuthorization { stream, id: stream_id })));
            }
            if is_datagram_mux {
                break Ok(Some(Box::new(DatagramMultiplexer {
                    stream,
                    id: stream_id,
                })));
            }
            break Ok(Some(Box::new(TcpConnection { stream, id: stream_id })));
        }
    }
}

impl downstream::StreamId for TcpConnection {
    fn id(&self) -> log_utils::IdChain<u64> {
        self.id.clone()
    }
}

impl downstream::AuthorizationRequest for TcpConnection {
    fn auth_info(&self) -> io::Result<authorization::Source> {
        self.stream.request().auth_info()
    }

    fn succeed_request(self: Box<Self>) -> io::Result<Option<downstream::AuthorizedRequest>> {
        Ok(Some(downstream::AuthorizedRequest::TcpConnect(self)))
    }

    fn fail_request(self: Box<Self>) {
        fail_auth_request(self.stream)
    }
}

impl downstream::PendingTcpConnectRequest for TcpConnection {
    fn destination(&self) -> io::Result<TcpDestination> {
        let request = self.stream.request();
        let authority = request.authority()?;

        Ok(match authority.as_str().parse() {
            Ok(a) => TcpDestination::Address(a),
            Err(_) => {
                let port = if request.request().method == http::Method::CONNECT {
                    authority.port_u16()
                        .ok_or_else(|| io::Error::new(
                            ErrorKind::Other,
                            format!("Unexpected authority port: request={:?}", request.request())
                        ))?
                } else {
                    authority.port_u16().unwrap_or(net_utils::PLAIN_HTTP_PORT_NUMBER)
                };

                TcpDestination::HostName((authority.host().to_string(), port))
            },
        })
    }

    fn succeed_request(self: Box<Self>) -> io::Result<(Box<dyn pipe::Source>, Box<dyn pipe::Sink>)> {
        if self.stream.request().request().method == http::Method::CONNECT {
            let (source, sink) = self.stream.split();
            return Ok((
                source.finalize(),
                sink.send_ok_response(false)?.into_pipe_sink(),
            ));
        }

        http_forwarded_stream::into_forwarded(self.stream)
    }

    fn fail_request(self: Box<Self>, error: io::Error) -> io::Result<()> {
        fail_request(self.stream, BAD_STATUS_CODE, vec![io_error_to_warn_header(Some(error))]);
        Ok(())
    }
}

impl downstream::StreamId for PendingAuthorization {
    fn id(&self) -> log_utils::IdChain<u64> {
        self.id.clone()
    }
}

impl downstream::AuthorizationRequest for PendingAuthorization {
    fn auth_info(&self) -> io::Result<authorization::Source> {
        self.stream.request().auth_info()
    }

    fn succeed_request(self: Box<Self>) -> io::Result<Option<downstream::AuthorizedRequest>> {
        self.stream.split().1.send_ok_response(true).map(|_| None)
    }

    fn fail_request(self: Box<Self>) {
        fail_auth_request(self.stream)
    }
}

impl downstream::StreamId for DatagramMultiplexer {
    fn id(&self) -> log_utils::IdChain<u64> {
        self.id.clone()
    }
}

impl downstream::AuthorizationRequest for DatagramMultiplexer {
    fn auth_info(&self) -> io::Result<authorization::Source> {
        self.stream.request().auth_info()
    }

    fn succeed_request(self: Box<Self>) -> io::Result<Option<downstream::AuthorizedRequest>> {
        Ok(Some(downstream::AuthorizedRequest::DatagramMultiplexer(self)))
    }

    fn fail_request(self: Box<Self>) {
        fail_auth_request(self.stream)
    }
}

impl downstream::PendingDatagramMultiplexerRequest for DatagramMultiplexer {
    fn succeed_request(self: Box<Self>) -> io::Result<downstream::DatagramPipeHalves> {
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
                    sink: sink.send_ok_response(false)?.into_datagram_sink(),
                    encoder: Box::new(http_udp_codec::Encoder::default()),
                }),
            )),
            ICMP_AUTHORITY => {
                Ok(downstream::DatagramPipeHalves::Icmp(
                    Box::new(DatagramDecoder {
                        source: source.finalize(),
                        decoder: Box::new(http_icmp_codec::Decoder::new()),
                        pending_bytes: Default::default(),
                    }),
                    Box::new(DatagramEncoder {
                        sink: sink.send_ok_response(false)?.into_datagram_sink(),
                        encoder: Box::new(http_icmp_codec::Encoder::default()),
                    }),
                ))
            },
            _ => unreachable!(),
        }
    }

    fn fail_request(self: Box<Self>, error: io::Error) -> io::Result<()> {
        fail_request(self.stream, BAD_STATUS_CODE, vec![io_error_to_warn_header(Some(error))]);
        Ok(())
    }
}

impl<D> downstream::StreamId for DatagramDecoder<D> {
    fn id(&self) -> log_utils::IdChain<u64> {
        self.source.id().clone()
    }
}

#[async_trait]
impl<D> datagram_pipe::Source for DatagramDecoder<D> {
    type Output = D;

    fn id(&self) -> log_utils::IdChain<u64> {
        self.source.id().clone()
    }

    async fn read(&mut self) -> io::Result<D> {
        loop {
            let chunk = match self.pending_bytes.pop_front() {
                None => match self.source.read().await? {
                    pipe::Data::Chunk(bytes) => bytes,
                    pipe::Data::Eof => return Err(io::Error::from(ErrorKind::UnexpectedEof)),
                }
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
        match self.encoder.encode_packet(&datagram) {
            None => {
                debug!("Couldn't encode datagram");
                Ok(datagram_pipe::SendStatus::Dropped)
            }
            Some(encoded) => self.sink.write(encoded),
        }
    }
}

fn io_error_to_warn_header(error: Option<io::Error>) -> (String, String) {
    (
        WARNING_HEADER_NAME.to_string(),
        error.map_or(
            Warning::CONNECTION_FAILED.make_header_value(),
            |e| match e {
                // for now, `ErrorKind::HostUnreachable` and `ErrorKind::NetworkUnreachable` are unstable
                e if e.raw_os_error() == Some(libc::ENETUNREACH)
                    || e.raw_os_error() == Some(libc::EHOSTUNREACH)
                => Warning::HOST_UNREACHABLE.make_header_value(),
                _ => match e.kind() {
                    ErrorKind::TimedOut => Warning::CONNECTION_TIMEDOUT.make_header_value(),
                    _ => Warning::CONNECTION_FAILED.make_header_value(),
                }
            }
        )
    )
}

fn fail_request(stream: Box<dyn http_codec::Stream>, status: StatusCode, extra_headers: Vec<(String, String)>) {
    let id = stream.id();
    if let Err(e) = stream.split().1.send_bad_response(status, extra_headers) {
        log_id!(debug, id, "Failed to send bad response: {}", e);
    }
}

fn fail_auth_request(stream: Box<dyn http_codec::Stream>) {
    fail_request(
        stream,
        AUTHORIZATION_FAILURE_STATUS_CODE,
        vec![(AUTHORIZATION_FAILURE_EXTRA_HEADER.0.to_string(), AUTHORIZATION_FAILURE_EXTRA_HEADER.1.to_string())],
    )
}
