use crate::forwarder::TcpConnector;
use crate::http_codec::HttpCodec;
use crate::net_utils::TcpDestination;
use crate::pipe::DuplexPipe;
use crate::tcp_forwarder::TcpForwarder;
use crate::tls_demultiplexer::Protocol;
use crate::{core, forwarder, http1_codec, http_codec, log_id, log_utils, pipe, tunnel};
use bytes::{Buf, BufMut, BytesMut};
use std::io;
use std::io::ErrorKind;
use std::net::Ipv4Addr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

static ORIGINAL_PROTOCOL_HEADER: http::HeaderName =
    http::HeaderName::from_static("x-original-protocol");
const H3_BUFFERED_BODY_LIMIT: usize = 2 * 1024 * 1024;

#[derive(Default)]
struct SessionManager {
    active_streams_num: AtomicUsize,
}

pub(crate) async fn listen(
    context: Arc<core::Context>,
    mut codec: Box<dyn HttpCodec>,
    sni: String,
    log_id: log_utils::IdChain<u64>,
) {
    let (mut shutdown_notification, _shutdown_completion) = {
        let shutdown = context.shutdown.lock().unwrap();
        (shutdown.notification_handler(), shutdown.completion_guard())
    };

    tokio::select! {
        x = shutdown_notification.wait() => {
            match x {
                Ok(_) => (),
                Err(e) => log_id!(debug, log_id, "Shutdown notification failure: {}", e),
            }
        },
        _ = listen_inner(context, codec.as_mut(), sni, &log_id) => (),
    }

    if let Err(e) = codec.graceful_shutdown().await {
        log_id!(debug, log_id, "Failed to shutdown HTTP session: {}", e);
    }
}

async fn listen_inner(
    context: Arc<core::Context>,
    codec: &mut dyn HttpCodec,
    sni: String,
    log_id: &log_utils::IdChain<u64>,
) {
    let manager = Arc::new(SessionManager::default());
    let timeout = context.settings.connection_establishment_timeout;
    loop {
        match tokio::time::timeout(timeout, codec.listen()).await {
            Ok(Ok(Some(x))) => {
                tokio::spawn({
                    let context = context.clone();
                    let manager = manager.clone();
                    let protocol = codec.protocol();
                    let sni = sni.clone();
                    let log_id = log_id.clone();
                    async move {
                        manager.active_streams_num.fetch_add(1, Ordering::AcqRel);
                        if let Err(e) = handle_stream(context, x, protocol, sni, &log_id).await {
                            log_id!(debug, log_id, "Request failed: {}", e);
                        }
                        manager.active_streams_num.fetch_sub(1, Ordering::AcqRel);
                    }
                });
            }
            Ok(Ok(None)) => {
                log_id!(trace, log_id, "Connection closed");
                break;
            }
            Ok(Err(ref e)) if e.kind() == ErrorKind::UnexpectedEof => {
                log_id!(trace, log_id, "Connection closed");
                break;
            }
            Ok(Err(e)) => {
                log_id!(debug, log_id, "Session error: {}", e);
                break;
            }
            Err(_elapsed) if manager.active_streams_num.load(Ordering::Acquire) > 0 => log_id!(
                trace,
                log_id,
                "Ignoring timeout due to there are some active streams"
            ),
            Err(_elapsed) => {
                log_id!(debug, log_id, "Closing due to timeout");
                if let Err(e) = codec.graceful_shutdown().await {
                    log_id!(debug, log_id, "Failed to shut down session: {}", e);
                }
                break;
            }
        }
    }
}

async fn handle_stream(
    context: Arc<core::Context>,
    stream: Box<dyn http_codec::Stream>,
    protocol: Protocol,
    sni: String,
    log_id: &log_utils::IdChain<u64>,
) -> io::Result<()> {
    let (request, respond) = stream.split();
    log_id!(trace, log_id, "Received request: {:?}", request.request());

    let forwarder = Box::new(TcpForwarder::new(context.clone()));
    let settings = context.settings.reverse_proxy.as_ref().unwrap();
    let (mut server_source, mut server_sink) = forwarder
        .connect(
            log_id.clone(),
            forwarder::TcpConnectionMeta {
                client_address: Ipv4Addr::UNSPECIFIED.into(),
                destination: TcpDestination::Address(settings.server_address),
                auth: None,
                tls_domain: sni,
                user_agent: None,
            },
        )
        .await
        .map_err(|e| match e {
            tunnel::ConnectionError::Io(e) => e,
            _ => io::Error::new(ErrorKind::Other, format!("{}", e)),
        })?;

    let mut request_headers = request.clone_request();
    let original_version = request_headers.version;
    match protocol {
        Protocol::Http1 => (),
        Protocol::Http2 => {
            request_headers.version = http::Version::HTTP_11;
        }
        Protocol::Http3 => {
            request_headers.version = http::Version::HTTP_11;
            if settings.h3_backward_compatibility
                && request_headers.method == http::Method::GET
                && request_headers.uri.path() == "/"
            {
                request_headers.method = http::Method::CONNECT;
            }
        }
    }
    request_headers.headers.insert(
        &ORIGINAL_PROTOCOL_HEADER,
        http::HeaderValue::from_static(protocol.as_str()),
    );

    let encoded = http1_codec::encode_request(&request_headers);
    log_id!(
        trace,
        log_id,
        "Sending translated request: {:?}",
        request_headers
    );
    server_sink.write_all(encoded).await?;

    let mut buffer = BytesMut::new();
    let (response, chunk, is_chunked) = loop {
        match server_source.read().await? {
            pipe::Data::Chunk(chunk) => {
                server_source.consume(chunk.len())?;
                buffer.put(chunk);
            }
            pipe::Data::Eof => {
                // Upstream closed before sending a valid HTTP response. Reply with 502
                // to avoid surfacing this as an H2 stream cancel to the client.
                return send_bad_gateway(respond, original_version);
            }
        }

        match http1_codec::decode_response(
            buffer,
            http1_codec::MAX_HEADERS_NUM,
            http1_codec::MAX_RAW_HEADERS_SIZE,
        )? {
            http1_codec::DecodeStatus::Partial(b) => buffer = b,
            http1_codec::DecodeStatus::Complete(mut h, tail) => {
                h.version = original_version; // restore the version in case it was not the same
                let transfer_encoding_raw = h
                    .headers
                    .get(http::header::TRANSFER_ENCODING)
                    .and_then(|x| x.to_str().ok())
                    .map(str::to_owned);
                let is_chunked = transfer_encoding_raw
                    .as_deref()
                    .is_some_and(|v| v.to_ascii_lowercase().contains("chunked"));
                if !matches!(protocol, Protocol::Http1) {
                    // Strip hop-by-hop headers that are invalid in HTTP/2 and HTTP/3.
                    h.headers.remove(http::header::CONNECTION);
                    h.headers.remove(http::header::TRANSFER_ENCODING);
                    h.headers.remove(http::header::UPGRADE);
                    h.headers.remove(http::header::TE);
                    h.headers.remove(http::header::TRAILER);
                    h.headers
                        .remove(http::HeaderName::from_static("keep-alive"));
                    h.headers
                        .remove(http::HeaderName::from_static("proxy-connection"));
                }
                break (h, tail.freeze(), is_chunked);
            }
        }
    };

    let content_length = response
        .headers
        .get(http::header::CONTENT_LENGTH)
        .and_then(|x| x.to_str().ok())
        .and_then(|x| x.parse::<usize>().ok());
    // H3 streaming is fragile in practice; buffer reasonably small bodies to avoid truncation.
    if matches!(protocol, Protocol::Http3)
        && content_length.is_some_and(|x| x <= H3_BUFFERED_BODY_LIMIT)
    {
        let total = content_length.unwrap();
        let mut body = BytesMut::with_capacity(total);
        let chunk_len = chunk.len();
        body.put(chunk);
        server_source.consume(chunk_len)?;

        let mut remaining = total.saturating_sub(chunk_len);
        while remaining > 0 {
            match server_source.read().await? {
                pipe::Data::Chunk(chunk) => {
                    server_source.consume(chunk.len())?;
                    let to_take = std::cmp::min(chunk.len(), remaining);
                    body.put(chunk.slice(..to_take));
                    remaining -= to_take;
                }
                pipe::Data::Eof => {
                    return Err(ErrorKind::UnexpectedEof.into());
                }
            }
        }

        let mut client_sink = respond.send_response(response, false)?.into_pipe_sink();
        write_all(&mut client_sink, body.freeze()).await?;
        client_sink.eof()?;
        return Ok(());
    }

    let chunk_len = chunk.len();

    if is_chunked && !matches!(protocol, Protocol::Http1) {
        async fn need_more(
            buffer: &mut BytesMut,
            server_source: &mut Box<dyn pipe::Source>,
            _log_id: &log_utils::IdChain<u64>,
        ) -> io::Result<bool> {
            match server_source.read().await? {
                pipe::Data::Chunk(chunk) => {
                    server_source.consume(chunk.len())?;
                    buffer.put(chunk);
                    Ok(false)
                }
                pipe::Data::Eof => Ok(true),
            }
        }

        // Decode chunked HTTP/1 body and stream raw bytes to the client.
        // For HTTP/2 and HTTP/3, buffer reasonably small responses so we can
        // set Content-Length and avoid relying on connection teardown signals.
        let buffer_for_length = !matches!(protocol, Protocol::Http1);
        let mut buffered_body = BytesMut::new();
        let mut respond_opt = Some(respond);
        let mut response_opt = Some(response);
        let mut client_sink: Option<Box<dyn pipe::Sink>> = None;
        let ensure_client_sink = |response_opt: &mut Option<http_codec::ResponseHeaders>,
                                  respond_opt: &mut Option<Box<dyn http_codec::PendingRespond>>,
                                  client_sink: &mut Option<Box<dyn pipe::Sink>>|
         -> io::Result<()> {
            if client_sink.is_some() {
                return Ok(());
            }
            let response = response_opt
                .take()
                .ok_or_else(|| io::Error::new(ErrorKind::Other, "missing response"))?;
            let respond = respond_opt
                .take()
                .ok_or_else(|| io::Error::new(ErrorKind::Other, "missing respond"))?;
            *client_sink = Some(respond.send_response(response, false)?.into_pipe_sink());
            Ok(())
        };

        let mut buffer = BytesMut::new();
        buffer.put(chunk);
        server_source.consume(chunk_len)?;
        loop {
            // Ensure we have a full chunk size line.
            let line_end = loop {
                if let Some(pos) = buffer
                    .windows(2)
                    .position(|w| w == b"\r\n")
                    .map(|p| p + 2)
                {
                    break pos;
                }
                let eof = need_more(&mut buffer, &mut server_source, log_id).await?;
                if eof {
                    return Err(ErrorKind::UnexpectedEof.into());
                }
            };

            let mut line = buffer.split_to(line_end);
            line.truncate(line.len().saturating_sub(2));
            let line = std::str::from_utf8(&line)
                .map_err(|e| io::Error::new(ErrorKind::InvalidData, e))?;
            let size_hex = line.split(';').next().unwrap_or_default().trim();
            let chunk_size = usize::from_str_radix(size_hex, 16)
                .map_err(|e| io::Error::new(ErrorKind::InvalidData, e))?;
            if chunk_size == 0 {
                // If there are no trailers, the stream ends with a single CRLF.
                if buffer.len() >= 2 && &buffer[..2] == b"\r\n" {
                    buffer.advance(2);
                } else {
                    // Consume trailers until CRLFCRLF or upstream EOF.
                    loop {
                        if buffer.windows(4).any(|w| w == b"\r\n\r\n") {
                            break;
                        }
                        let eof = need_more(&mut buffer, &mut server_source, log_id).await?;
                        if eof {
                            break;
                        }
                    }
                }
                if buffer_for_length && client_sink.is_none() {
                    if let Some(resp) = response_opt.as_mut() {
                        resp.headers.insert(
                            http::header::CONTENT_LENGTH,
                            buffered_body.len().into(),
                        );
                    }
                    ensure_client_sink(&mut response_opt, &mut respond_opt, &mut client_sink)?;
                    if let Some(sink) = client_sink.as_mut() {
                        write_all(sink, buffered_body.split().freeze()).await?;
                    }
                }
                ensure_client_sink(&mut response_opt, &mut respond_opt, &mut client_sink)?;
                if let Some(sink) = client_sink.as_mut() {
                    sink.eof()?;
                }
                return Ok(());
            }

            // Ensure we have the full chunk plus its trailing CRLF.
            while buffer.len() < chunk_size + 2 {
                let eof = need_more(&mut buffer, &mut server_source, log_id).await?;
                if eof {
                    return Err(ErrorKind::UnexpectedEof.into());
                }
            }

            let data = buffer.split_to(chunk_size).freeze();
            let _ = buffer.split_to(2);

            if buffer_for_length && client_sink.is_none() {
                buffered_body.put(data.clone());
                if buffered_body.len() > H3_BUFFERED_BODY_LIMIT {
                    // Fall back to streaming without a known length.
                    ensure_client_sink(&mut response_opt, &mut respond_opt, &mut client_sink)?;
                    if let Some(sink) = client_sink.as_mut() {
                        write_all(sink, buffered_body.split().freeze()).await?;
                    }
                }
                continue;
            }

            ensure_client_sink(&mut response_opt, &mut respond_opt, &mut client_sink)?;
            if let Some(sink) = client_sink.as_mut() {
                write_all(sink, data).await?;
            }
        }
    }

    let mut client_sink = respond.send_response(response, false)?.into_pipe_sink();
    write_all(&mut client_sink, chunk).await?;
    server_source.consume(chunk_len)?;

    if let Some(mut remaining) = content_length.and_then(|x| x.checked_sub(chunk_len)) {
        log_id!(
            debug,
            log_id,
            "Reverse proxy fixed-size body: remaining={} bytes after initial send",
            remaining
        );
        while remaining > 0 {
            match server_source.read().await? {
                pipe::Data::Chunk(chunk) => {
                    server_source.consume(chunk.len())?;
                    let to_send = std::cmp::min(chunk.len(), remaining);
                    write_all(&mut client_sink, chunk.slice(..to_send)).await?;
                    remaining -= to_send;
                }
                pipe::Data::Eof => break,
            }
        }
        if let Err(e) = client_sink.eof() {
            log_id!(debug, log_id, "Failed to close client stream: {}", e);
        } else {
            log_id!(debug, log_id, "Reverse proxy client stream closed");
        }
        return Ok(());
    }

    let mut pipe = DuplexPipe::new(
        (
            pipe::SimplexDirection::Outgoing,
            request.finalize(),
            server_sink,
        ),
        (pipe::SimplexDirection::Incoming, server_source, client_sink),
        |_, _| (),
    );

    match pipe
        .exchange(context.settings.tcp_connections_timeout)
        .await
    {
        Ok(()) => Ok(()),
        // HTTP/2 (and sometimes HTTP/3) can surface graceful stream closure
        // as UnexpectedEof once the response has already been delivered.
        Err(e) if e.kind() == ErrorKind::UnexpectedEof => Ok(()),
        Err(e) => Err(e),
    }
}

async fn write_all(sink: &mut Box<dyn pipe::Sink>, mut data: bytes::Bytes) -> io::Result<()> {
    while !data.is_empty() {
        let before = data.len();
        data = sink.write(data)?;
        if data.len() == before || !data.is_empty() {
            sink.wait_writable().await?;
        }
    }
    Ok(())
}

fn send_bad_gateway(
    respond: Box<dyn http_codec::PendingRespond>,
    version: http::Version,
) -> io::Result<()> {
    let response = http::Response::builder()
        .status(http::StatusCode::BAD_GATEWAY)
        .version(version)
        .body(())
        .map_err(|e| io::Error::new(ErrorKind::Other, format!("bad gateway: {}", e)))?;
    let (parts, _) = response.into_parts();
    respond.send_response(parts, true).map(|_| ())
}
