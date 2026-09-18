use crate::http_codec::{RequestHeaders, ResponseHeaders};
use crate::{http_codec, log_id, log_utils, pipe};
use async_trait::async_trait;
use bytes::{BufMut, Bytes, BytesMut};
use std::collections::HashSet;
use std::io;
use std::io::ErrorKind;

const INITIAL_RESPONSE_HEADERS_BUFFER_SIZE: usize = 32;
const MAX_RESPONSE_HEADERS_NUM: usize = 128;
const ENCODED_CHUNK_SUFFIX: &str = "\r\n";

/// Wrap the `stream` with a non-CONNECT request into a wrapper which forwards the request
/// to a peer as a plain HTTP/1 one
pub(crate) fn into_forwarded(
    stream: Box<dyn http_codec::Stream>,
) -> io::Result<(Box<dyn pipe::Source>, Box<dyn pipe::Sink>)> {
    let (request, respond) = stream.split();
    let method = request.request().method.clone();
    let version = request.request().version;

    let (serialized_request, body_length) = match serialize_request(request.request()) {
        Ok(x) => x,
        Err(e) => {
            let _ = respond.send_bad_response(http::StatusCode::BAD_REQUEST, vec![]);
            return Err(e);
        }
    };

    let id = request.id();

    Ok((
        Box::new(ForwardedStreamSource {
            state: SourceState::WantSendRequest(SourceWantSendRequest {
                request,
                serialized_request,
                body_length,
            }),
            skip_consume_bytes: 0,
            id: id.clone(),
        }),
        Box::new(ForwardedStreamSink {
            state: SinkState::WaitingResponse(SinkWaitingResponse {
                headers_buffer: BytesMut::new(),
                request_method: method,
                request_version: version,
                respond,
            }),
            fake_unsent: false,
            id,
        }),
    ))
}

struct ForwardedStreamSource {
    state: SourceState,
    /// Needed to not over-consume bytes sent as part of HTTP/1-specifics (like request headers and
    /// chunk markers)
    skip_consume_bytes: usize,
    id: log_utils::IdChain<u64>,
}

#[derive(Debug)]
enum BodyLength {
    Determined(u64),
    Chunked,
}

struct SourceWantSendRequest {
    request: Box<dyn http_codec::PendingRequest>,
    serialized_request: Bytes,
    body_length: BodyLength,
}

struct SourceTransferringBody {
    source: Box<dyn pipe::Source>,
    body_length: BodyLength,
    sent_bytes: u64,
}

enum SourceState {
    WantSendRequest(SourceWantSendRequest),
    TransferringBody(SourceTransferringBody),
    Done,
}

struct ForwardedStreamSink {
    state: SinkState,
    fake_unsent: bool,
    id: log_utils::IdChain<u64>,
}

struct SinkWaitingResponse {
    headers_buffer: BytesMut,
    request_method: http::Method,
    request_version: http::Version,
    respond: Box<dyn http_codec::PendingRespond>,
}

struct SinkTransferringBodyNonEncoded {
    sink: Box<dyn pipe::Sink>,
    body_length: Option<u64>,
    sent_bytes: u64,
}

struct SinkWaitingChunkPrefix {
    buffer: BytesMut,
    sink: Box<dyn pipe::Sink>,
}

struct SinkTransferringBodyChunked {
    sink: Box<dyn pipe::Sink>,
    remaining_chunk_size: Option<u64>,
}

struct SinkWaitingChunkSuffix {
    buffer: BytesMut,
    terminating_chunk: bool,
    sink: Box<dyn pipe::Sink>,
}

enum SinkState {
    Idle,
    WaitingResponse(SinkWaitingResponse),
    TransferringBodyNonEncoded(SinkTransferringBodyNonEncoded),
    WaitingChunkPrefix(SinkWaitingChunkPrefix),
    TransferringBodyChunked(SinkTransferringBodyChunked),
    WaitingChunkSuffix(SinkWaitingChunkSuffix),
}

#[async_trait]
impl pipe::Source for ForwardedStreamSource {
    fn id(&self) -> log_utils::IdChain<u64> {
        self.id.clone()
    }

    async fn read(&mut self) -> io::Result<pipe::Data> {
        match &self.state {
            SourceState::WantSendRequest(_) => return self.on_read_request(),
            // workaround to please the compiler, as it does not allow to `await` since
            // the reference to the state is dropped later
            // @todo: make it better
            SourceState::TransferringBody(_) => (),
            SourceState::Done => return Ok(pipe::Data::Eof),
        }

        assert!(matches!(self.state, SourceState::TransferringBody(_)));
        self.read_body().await
    }

    fn consume(&mut self, mut size: usize) -> io::Result<()> {
        if self.skip_consume_bytes > 0 {
            let to_sub = std::cmp::min(self.skip_consume_bytes, size);
            self.skip_consume_bytes -= to_sub;
            size -= to_sub;
            if size == 0 {
                return Ok(());
            }
        }

        match &mut self.state {
            SourceState::TransferringBody(x) => x.source.consume(size),
            _ => Ok(()),
        }
    }
}

impl ForwardedStreamSource {
    fn on_read_request(&mut self) -> io::Result<pipe::Data> {
        let state = match std::mem::replace(&mut self.state, SourceState::Done) {
            SourceState::WantSendRequest(x) => x,
            _ => unreachable!(),
        };

        self.skip_consume_bytes = state.serialized_request.len();
        self.state = match state.body_length {
            BodyLength::Determined(0) => SourceState::Done,
            body_length => SourceState::TransferringBody(SourceTransferringBody {
                source: state.request.finalize(),
                body_length,
                sent_bytes: 0,
            }),
        };

        Ok(pipe::Data::Chunk(state.serialized_request))
    }

    async fn read_body(&mut self) -> io::Result<pipe::Data> {
        let mut state = match std::mem::replace(&mut self.state, SourceState::Done) {
            SourceState::TransferringBody(x) => x,
            _ => unreachable!(),
        };

        let result = state.source.read().await?;

        self.state = SourceState::TransferringBody(state);
        let state = match &mut self.state {
            SourceState::TransferringBody(x) => x,
            _ => unreachable!(),
        };

        match (result, &state.body_length) {
            (pipe::Data::Chunk(mut bytes), BodyLength::Determined(n)) if state.sent_bytes < *n => {
                let to_send = std::cmp::min(bytes.len() as u64, n - state.sent_bytes);
                state.sent_bytes += to_send;
                Ok(pipe::Data::Chunk(bytes.split_to(to_send as usize)))
            }
            (pipe::Data::Chunk(_) | pipe::Data::Eof, BodyLength::Determined(_)) => {
                self.state = SourceState::Done;
                Ok(pipe::Data::Eof)
            }
            (pipe::Data::Chunk(bytes), BodyLength::Chunked) => {
                state.sent_bytes += bytes.len() as u64;
                Ok(pipe::Data::Chunk(bytes))
            }
            (pipe::Data::Eof, BodyLength::Chunked) => {
                self.state = SourceState::Done;
                Ok(pipe::Data::Eof)
            }
        }
    }
}

#[async_trait]
impl pipe::Sink for ForwardedStreamSink {
    fn id(&self) -> log_utils::IdChain<u64> {
        self.id.clone()
    }

    fn write(&mut self, data: Bytes) -> io::Result<Bytes> {
        match &self.state {
            SinkState::Idle => Err(io::Error::other("Invalid state")),
            SinkState::WaitingResponse(_) => match self.on_response_headers_chunk(data) {
                Err(e) if matches!(self.state, SinkState::WaitingResponse(_)) => {
                    match std::mem::replace(&mut self.state, SinkState::Idle) {
                        SinkState::WaitingResponse(x) => {
                            let _ = x
                                .respond
                                .send_bad_response(http::StatusCode::BAD_GATEWAY, vec![]);
                        }
                        _ => unreachable!(),
                    }
                    Err(e)
                }
                x => x,
            },
            SinkState::TransferringBodyNonEncoded(_) => self.on_non_encoded_chunk(data),
            SinkState::WaitingChunkPrefix(_) => self.on_encoded_chunk_prefix(data),
            SinkState::TransferringBodyChunked(_) => self.on_encoded_chunk(data),
            SinkState::WaitingChunkSuffix(_) => self.on_encoded_chunk_suffix(data),
        }
    }

    fn eof(&mut self) -> io::Result<()> {
        match std::mem::replace(&mut self.state, SinkState::Idle) {
            SinkState::Idle => Ok(()),
            SinkState::WaitingResponse(x) => x
                .respond
                .send_bad_response(http::StatusCode::BAD_GATEWAY, vec![]),
            SinkState::TransferringBodyNonEncoded(mut x) => x.sink.eof(),
            SinkState::WaitingChunkPrefix(mut x) => x.sink.eof(),
            SinkState::TransferringBodyChunked(mut x) => x.sink.eof(),
            SinkState::WaitingChunkSuffix(mut x) => x.sink.eof(),
        }
    }

    async fn wait_writable(&mut self) -> io::Result<()> {
        if self.fake_unsent {
            self.fake_unsent = false;
            return Ok(());
        }

        match &mut self.state {
            SinkState::Idle | SinkState::WaitingResponse(_) => {
                Err(io::Error::other("Invalid state"))
            }
            SinkState::WaitingChunkPrefix(_) | SinkState::WaitingChunkSuffix(_) => Ok(()),
            SinkState::TransferringBodyNonEncoded(x) => x.sink.wait_writable().await,
            SinkState::TransferringBodyChunked(x) => x.sink.wait_writable().await,
        }
    }

    async fn flush(&mut self) -> io::Result<()> {
        let sink = match self.state {
            SinkState::Idle | SinkState::WaitingResponse(_) => return Ok(()),
            SinkState::TransferringBodyNonEncoded(ref mut x) => &mut x.sink,
            SinkState::WaitingChunkPrefix(ref mut x) => &mut x.sink,
            SinkState::TransferringBodyChunked(ref mut x) => &mut x.sink,
            SinkState::WaitingChunkSuffix(ref mut x) => &mut x.sink,
        };

        sink.flush().await
    }
}

impl ForwardedStreamSink {
    fn on_response_headers_chunk(&mut self, data: Bytes) -> io::Result<Bytes> {
        let state = match &mut self.state {
            SinkState::WaitingResponse(x) => x,
            _ => unreachable!(),
        };

        let ((response, body_length), tail) = match state.parse_response(data, &self.id)? {
            (Some(x), tail) => (x, tail),
            (None, tail) => return Ok(tail),
        };

        if (100..200).contains(&response.status.as_u16()) {
            state.respond.send_intermediate_response(response)?;
            return Ok(tail);
        }

        let state = match std::mem::replace(&mut self.state, SinkState::Idle) {
            SinkState::WaitingResponse(x) => x,
            _ => unreachable!(),
        };

        let sink = state.respond.send_response(
            response,
            matches!(body_length, Some(BodyLength::Determined(0))),
        )?;

        self.state = match body_length {
            Some(BodyLength::Chunked) => SinkState::WaitingChunkPrefix(SinkWaitingChunkPrefix {
                buffer: Default::default(),
                sink: sink.into_pipe_sink(),
            }),
            None | Some(BodyLength::Determined(_)) => {
                SinkState::TransferringBodyNonEncoded(SinkTransferringBodyNonEncoded {
                    sink: sink.into_pipe_sink(),
                    body_length: body_length.map(|x| match x {
                        BodyLength::Determined(x) => x,
                        BodyLength::Chunked => unreachable!(),
                    }),
                    sent_bytes: 0,
                })
            }
        };

        self.fake_unsent = !tail.is_empty();
        Ok(tail)
    }

    fn on_non_encoded_chunk(&mut self, mut data: Bytes) -> io::Result<Bytes> {
        log_id!(trace, self.id, "Non encoded chunk: {} bytes", data.len());
        let state = match &mut self.state {
            SinkState::TransferringBodyNonEncoded(x) => x,
            _ => unreachable!(),
        };

        let to_send = match state.body_length {
            None => data.len(),
            Some(n) => {
                if n <= state.sent_bytes {
                    return Err(ErrorKind::UnexpectedEof.into());
                }
                std::cmp::min(data.len(), (n - state.sent_bytes) as usize)
            }
        };

        if to_send == 0 {
            return Ok(data);
        }

        let unsent = state.sink.write(data.slice(..to_send))?;
        // Only the bytes handed to the sink count: the whole input length can carry bytes
        // past the declared body, which would overshoot `body_length` and skip `eof()`.
        state.sent_bytes += (to_send - unsent.len()) as u64;

        if Some(state.sent_bytes) == state.body_length {
            assert!(unsent.is_empty());
            assert_eq!(data.len(), to_send);
            state.sink.eof()?;
        }

        Ok(data.split_off(to_send - unsent.len()))
    }

    fn on_encoded_chunk_prefix(&mut self, data: Bytes) -> io::Result<Bytes> {
        let state = match &mut self.state {
            SinkState::WaitingChunkPrefix(x) => x,
            _ => unreachable!(),
        };

        let mut data = if state.buffer.is_empty() {
            data
        } else {
            state.buffer.extend_from_slice(&data);
            std::mem::take(&mut state.buffer).freeze()
        };

        let (chunk_size, tail) = match httparse::parse_chunk_size(&data) {
            Ok(httparse::Status::Complete((pos, chunk_size))) => {
                log_id!(trace, self.id, "Encoded chunk size: {} bytes", chunk_size);
                (chunk_size, data.split_off(pos))
            }
            Ok(httparse::Status::Partial) => {
                state.buffer = BytesMut::from(data.as_ref());
                return Ok(Bytes::new());
            }
            Err(httparse::InvalidChunkSize) => {
                return Err(io::Error::other("Invalid encoded chunk size"))
            }
        };

        if chunk_size == 0 {
            self.state = SinkState::WaitingChunkSuffix(SinkWaitingChunkSuffix {
                buffer: Default::default(),
                terminating_chunk: true,
                sink: match std::mem::replace(&mut self.state, SinkState::Idle) {
                    SinkState::WaitingChunkPrefix(x) => x.sink,
                    _ => unreachable!(),
                },
            });
        } else {
            self.state = SinkState::TransferringBodyChunked(SinkTransferringBodyChunked {
                sink: match std::mem::replace(&mut self.state, SinkState::Idle) {
                    SinkState::WaitingChunkPrefix(x) => x.sink,
                    _ => unreachable!(),
                },
                remaining_chunk_size: Some(chunk_size),
            });
        }

        self.fake_unsent = !tail.is_empty();
        Ok(tail)
    }

    fn on_encoded_chunk(&mut self, mut data: Bytes) -> io::Result<Bytes> {
        let mut state = match std::mem::replace(&mut self.state, SinkState::Idle) {
            SinkState::TransferringBodyChunked(x) => x,
            _ => unreachable!(),
        };

        let to_send =
            std::cmp::min(data.len() as u64, state.remaining_chunk_size.unwrap()) as usize;
        let unsent = state.sink.write(data.slice(..to_send))?;

        let remaining = state
            .remaining_chunk_size
            .take()
            .unwrap()
            .saturating_sub(to_send as u64);
        log_id!(
            trace,
            self.id,
            "Encoded chunk: {} bytes (remaining {} bytes)",
            to_send,
            remaining
        );
        if remaining > 0 {
            state.remaining_chunk_size = Some(remaining);
        } else {
            self.state = SinkState::WaitingChunkSuffix(SinkWaitingChunkSuffix {
                buffer: BytesMut::with_capacity(ENCODED_CHUNK_SUFFIX.len()),
                terminating_chunk: false,
                sink: state.sink,
            });
        }
        self.fake_unsent = !data.is_empty();

        Ok(data.split_off(to_send - unsent.len()))
    }

    fn on_encoded_chunk_suffix(&mut self, mut data: Bytes) -> io::Result<Bytes> {
        let mut state = match std::mem::replace(&mut self.state, SinkState::Idle) {
            SinkState::WaitingChunkSuffix(x) => x,
            _ => unreachable!(),
        };

        let suffix = if state.buffer.is_empty() {
            data.split_to(std::cmp::min(data.len(), ENCODED_CHUNK_SUFFIX.len()))
        } else {
            let to_read =
                std::cmp::min(data.len(), ENCODED_CHUNK_SUFFIX.len() - state.buffer.len());
            state.buffer.extend_from_slice(&data.split_to(to_read));
            std::mem::take(&mut state.buffer).freeze()
        };

        if !ENCODED_CHUNK_SUFFIX.as_bytes().starts_with(&suffix) {
            return Err(io::Error::other("Invalid encoded chunk suffix"));
        }

        if suffix.len() < ENCODED_CHUNK_SUFFIX.len() {
            state.buffer = BytesMut::from(suffix.as_ref());
            self.state = SinkState::WaitingChunkSuffix(state);
        } else if state.terminating_chunk {
            if !data.is_empty() {
                log_id!(
                    debug,
                    self.id,
                    "Dropping non-processed {} bytes coming after terminating encoded chunk",
                    data.len()
                );
            }
            data = Bytes::new();
            state.sink.eof()?;
        } else {
            self.state = SinkState::WaitingChunkPrefix(SinkWaitingChunkPrefix {
                buffer: Default::default(),
                sink: state.sink,
            });
        }

        Ok(data)
    }
}

impl SinkWaitingResponse {
    #[allow(clippy::type_complexity)]
    fn parse_response(
        &mut self,
        data: Bytes,
        log_id: &log_utils::IdChain<u64>,
    ) -> io::Result<(Option<(ResponseHeaders, Option<BodyLength>)>, Bytes)> {
        let mut data = if self.headers_buffer.is_empty() {
            data
        } else {
            self.headers_buffer.extend_from_slice(&data);
            std::mem::take(&mut self.headers_buffer).freeze()
        };

        let mut parse_headers_buffer =
            vec![httparse::EMPTY_HEADER; INITIAL_RESPONSE_HEADERS_BUFFER_SIZE];
        loop {
            let mut response = httparse::Response::new(parse_headers_buffer.as_mut());
            match response.parse(data.as_ref()) {
                Ok(httparse::Status::Complete(pos)) => {
                    log_id!(trace, log_id, "Received response: {:?}", response);
                    return Ok((Some(self.convert_response(response)?), data.split_off(pos)));
                }
                Ok(httparse::Status::Partial) => {
                    self.headers_buffer = BytesMut::from(data.as_ref());
                    return Ok((None, Bytes::new()));
                }
                Err(httparse::Error::TooManyHeaders)
                    if parse_headers_buffer.len() < MAX_RESPONSE_HEADERS_NUM =>
                {
                    parse_headers_buffer
                        .resize(2 * parse_headers_buffer.len(), httparse::EMPTY_HEADER)
                }
                Err(e) => return Err(io::Error::other(format!("Failed to parse response: {}", e))),
            }
        }
    }

    fn convert_response(
        &self,
        response: httparse::Response,
    ) -> io::Result<(ResponseHeaders, Option<BodyLength>)> {
        let mut builder = http::response::Response::builder()
            .version(self.request_version)
            .status(response.code.unwrap());
        let mut body_length = None;
        let mut drop_headers =
            HashSet::from(["proxy-connection", "keep-alive", "upgrade"].map(|h| h.to_string()));
        for h in response.headers {
            match (h.name.to_ascii_lowercase().as_str(), self.request_version) {
                (x, _) if drop_headers.contains(x) => (),
                ("connection", _) => {
                    if let Ok(x) = std::str::from_utf8(h.value) {
                        drop_headers.extend(
                            x.split(',')
                                .filter(|x| *x != "close")
                                .map(|x| x.trim().to_lowercase()),
                        );
                    }
                }
                ("transfer-encoding", http::Version::HTTP_2 | http::Version::HTTP_3) => {
                    if h.value == "chunked".as_bytes() {
                        body_length = Some(BodyLength::Chunked);
                    }
                    drop_headers.insert("content-length".to_string());
                    drop_headers.insert("transfer-encoding".to_string());
                }
                (x, _) => {
                    if body_length.is_none() && x == "content-length" {
                        body_length = Some(BodyLength::Determined(
                            std::str::from_utf8(h.value)
                                .map_err(|e| {
                                    io::Error::other(format!(
                                        "Invalid Content-Length header value: {:?}, error={}",
                                        h.value, e
                                    ))
                                })?
                                .parse::<u64>()
                                .map_err(|e| {
                                    io::Error::other(format!(
                                        "Invalid Content-Length header value: {:?}, error={}",
                                        h.value, e
                                    ))
                                })?,
                        ));
                    }
                    builder = builder.header(h.name.trim_end(), h.value)
                }
            }
        }

        if self.request_method == http::Method::HEAD
            || response
                .code
                .is_some_and(|x| (100..200).contains(&x) || x == 204 || x == 304)
        {
            body_length = Some(BodyLength::Determined(0));
        }

        let response = builder
            .body(())
            .map_err(|e| io::Error::other(format!("Invalid response: {}", e)))?
            .into_parts()
            .0;

        Ok((response, body_length))
    }
}

fn version_major_digit(v: http::Version) -> u32 {
    match v {
        http::Version::HTTP_09 => 0,
        _ => 1,
    }
}

fn version_minor_digit(v: http::Version) -> u32 {
    match v {
        http::Version::HTTP_09 => 0,
        http::Version::HTTP_10 => 0,
        http::Version::HTTP_11 => 1,
        http::Version::HTTP_2 => 1,
        http::Version::HTTP_3 => 1,
        _ => unreachable!(),
    }
}

fn serialize_request(request: &RequestHeaders) -> io::Result<(Bytes, BodyLength)> {
    let mut serialized = BytesMut::new();
    serialized.put(
        format!(
            "{} {} HTTP/{}.{}\r\n",
            request.method.as_str(),
            if request.method != http::Method::OPTIONS {
                request
                    .uri
                    .path_and_query()
                    .map_or(request.uri.path(), |x| x.as_str())
            } else {
                "*"
            },
            version_major_digit(request.version),
            version_minor_digit(request.version),
        )
        .as_bytes(),
    );

    let target_host = request.uri.authority().ok_or_else(|| {
        io::Error::other(format!("Request URI lacks host name: {:?}", request.uri))
    })?;

    let mut host_inserted = false;
    let mut body_length = None;
    for (name, value) in &request.headers {
        match name.as_str() {
            "proxy-authorization" => (),
            "proxy-connection" => (),
            "host" => {
                if !host_inserted {
                    host_inserted = true;
                    serialized.put("host: ".as_bytes());
                    serialized.put(target_host.as_str().as_bytes());
                    serialized.put("\r\n".as_bytes());
                } else {
                    return Err(io::Error::other("Request has multiple Host headers"));
                }
            }
            name => {
                match (name, &body_length) {
                    ("content-length", None) => {
                        body_length = Some(BodyLength::Determined(
                            value
                                .to_str()
                                .map_err(|_| {
                                    io::Error::other("Invalid Content-Length header value")
                                })?
                                .parse::<u64>()
                                .map_err(|_| {
                                    io::Error::other("Invalid Content-Length header value")
                                })?,
                        ))
                    }
                    ("content-length", Some(BodyLength::Determined(_))) => {
                        return Err(io::Error::other(
                            "Request has multiple Content-Length headers",
                        ))
                    }
                    ("transfer-encoding", _) if value == "chunked" => {
                        body_length = Some(BodyLength::Chunked)
                    }
                    _ => (),
                }

                serialized.put(format!("{}: ", name).as_bytes());
                serialized.put(value.as_bytes());
                serialized.put("\r\n".as_bytes());
            }
        }
    }

    if !host_inserted {
        serialized.put("host: ".as_bytes());
        serialized.put(target_host.as_str().as_bytes());
        serialized.put("\r\n".as_bytes());
    }

    serialized.put("\r\n".as_bytes());

    body_length = if request.method == http::Method::HEAD {
        Some(BodyLength::Determined(0))
    } else {
        body_length.or(match request.version {
            http::Version::HTTP_2 | http::Version::HTTP_3 => Some(BodyLength::Chunked),
            _ => None,
        })
    };

    Ok((
        serialized.freeze(),
        body_length.unwrap_or(BodyLength::Determined(0)),
    ))
}
