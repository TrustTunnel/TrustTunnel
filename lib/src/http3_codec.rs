use crate::http_codec::{HttpCodec, RequestHeaders, ResponseHeaders};
use crate::quic_multiplexer::{QuicSocket, QuicSocketEvent};
use crate::tls_demultiplexer::Protocol;
use crate::{datagram_pipe, http_codec, log_id, log_utils, net_utils, pipe};
use async_trait::async_trait;
use bytes::Bytes;
use std::collections::HashMap;
use std::io;
use std::io::ErrorKind;
use std::net::IpAddr;
use std::sync::Arc;
use tokio::sync::mpsc;

pub(crate) struct Http3Codec {
    socket: Arc<QuicSocket>,
    streams: HashMap<u64, Stream>,
    /// Receives messages from [`StreamSource.codec_tx`] and [`StreamSink.codec_tx`]
    stream_rx: mpsc::UnboundedReceiver<StreamMessage>,
    /// See [`StreamSource.codec_tx`] and [`StreamSink.codec_tx`]
    codec_tx: Arc<mpsc::UnboundedSender<StreamMessage>>,
    parent_id_chain: log_utils::IdChain<u64>,
}

enum StreamMessage {
    WaitingWritable(/* stream ID */ u64),
    /// stream ID, shutdown direction (`None` means both directions)
    Shutdown(u64, Option<quiche::Shutdown>),
}

struct Stream {
    /// Sends messages to [`StreamSource.readable_event_rx`]
    readable_event_tx: mpsc::Sender<()>,
    /// Sends messages to [`StreamSink.writable_event_rx`]
    writable_event_tx: mpsc::Sender<()>,
    read_shutdown: bool,
    write_shutdown: bool,
}

struct DetachedStream {
    source: StreamSource,
    sink: StreamSink,
}

struct StreamSource {
    stream_id: u64,
    request: RequestHeaders,
    socket: Arc<QuicSocket>,
    /// Receives messages from [`Stream.readable_event_tx`]
    readable_event_rx: mpsc::Receiver<()>,
    /// Sends messages to [`Http3Codec.stream_rx`]
    codec_tx: Arc<mpsc::UnboundedSender<StreamMessage>>,
    id: log_utils::IdChain<u64>,
}

struct StreamSink {
    stream_id: u64,
    socket: Arc<QuicSocket>,
    /// Receives messages from [`Stream.writable_event_tx`]
    writable_event_rx: mpsc::Receiver<()>,
    /// Sends messages to [`Http3Codec.stream_rx`]
    codec_tx: Arc<mpsc::UnboundedSender<StreamMessage>>,
    /// Equals to [`net_utils::MIN_USABLE_QUIC_STREAM_CAPACITY`] by default.
    /// In some cases may be assigned to different values
    /// (see [`StreamSink::wait_writable()`]) to avoid busy loops.
    data_frame_overhead: usize,
    id: log_utils::IdChain<u64>,
}

impl Http3Codec {
    pub fn new(socket: QuicSocket, parent_id_chain: log_utils::IdChain<u64>) -> Self {
        let (tx, rx) = mpsc::unbounded_channel();

        Self {
            socket: Arc::new(socket),
            streams: HashMap::new(),
            stream_rx: rx,
            codec_tx: Arc::new(tx),
            parent_id_chain,
        }
    }

    fn on_stream_message(&mut self, message: StreamMessage) -> io::Result<()> {
        match message {
            StreamMessage::WaitingWritable(stream_id) => {
                self.socket.notify_stream_waiting_writable(stream_id);
                Ok(())
            }
            StreamMessage::Shutdown(stream_id, direction) => {
                self.on_stream_shutdown(stream_id, direction)
            }
        }
    }

    fn on_stream_shutdown(
        &mut self,
        stream_id: u64,
        direction: Option<quiche::Shutdown>,
    ) -> io::Result<()> {
        let stream = self
            .streams
            .get_mut(&stream_id)
            .ok_or_else(|| io::Error::from(ErrorKind::NotFound))?;

        let (close_read, close_write) = match direction {
            Some(quiche::Shutdown::Read) => (true, false),
            Some(quiche::Shutdown::Write) => (false, true),
            None => (true, true),
        };

        if close_read && !stream.read_shutdown {
            self.socket
                .shutdown_stream(stream_id, quiche::Shutdown::Read);
            stream.read_shutdown = true;
        }
        if close_write && !stream.write_shutdown {
            self.socket
                .shutdown_stream(stream_id, quiche::Shutdown::Write);
            stream.write_shutdown = true;
        }

        if stream.read_shutdown && stream.write_shutdown {
            self.streams.remove(&stream_id);
        }

        Ok(())
    }

    fn on_socket_event(
        &mut self,
        event: QuicSocketEvent,
    ) -> io::Result<Option<Box<dyn http_codec::Stream>>> {
        match event {
            QuicSocketEvent::Request(stream_id, request) => {
                self.on_request(stream_id, *request).map(Some)
            }
            QuicSocketEvent::Readable(stream_id) => {
                self.on_stream_readable(stream_id).map(|_| None)
            }
            QuicSocketEvent::Writable(streams) => {
                self.notify_writable_streams(streams);
                Ok(None)
            }
            QuicSocketEvent::Close(stream_id) => {
                // Client finished sending request body; keep write side open for response.
                let _ = self.on_stream_shutdown(stream_id, Some(quiche::Shutdown::Read));
                Ok(None)
            }
        }
    }

    fn on_request(
        &mut self,
        stream_id: u64,
        request: RequestHeaders,
    ) -> io::Result<Box<dyn http_codec::Stream>> {
        let (readable_tx, readable_rx) = mpsc::channel(1);
        let (writable_tx, writable_rx) = mpsc::channel(1);

        let id = self.parent_id_chain.extended(log_utils::IdItem::new(
            log_utils::CONNECTION_ID_FMT,
            stream_id,
        ));

        self.streams.insert(
            stream_id,
            Stream {
                readable_event_tx: readable_tx,
                writable_event_tx: writable_tx,
                read_shutdown: false,
                write_shutdown: false,
            },
        );

        Ok(Box::new(DetachedStream {
            source: StreamSource {
                stream_id,
                request,
                socket: self.socket.clone(),
                readable_event_rx: readable_rx,
                codec_tx: self.codec_tx.clone(),
                id: id.clone(),
            },
            sink: StreamSink {
                stream_id,
                socket: self.socket.clone(),
                writable_event_rx: writable_rx,
                codec_tx: self.codec_tx.clone(),
                data_frame_overhead: net_utils::MIN_USABLE_QUIC_STREAM_CAPACITY,
                id,
            },
        }))
    }

    fn on_stream_readable(&mut self, stream_id: u64) -> io::Result<()> {
        let stream = self
            .streams
            .get_mut(&stream_id)
            .ok_or_else(|| io::Error::from(ErrorKind::NotFound))?;

        match stream.readable_event_tx.try_send(()) {
            // `Full` is not considered as an error in this case, as the stream does not need
            // multiple `readable` messages in the queue
            Ok(_) | Err(mpsc::error::TrySendError::Full(_)) => Ok(()),
            Err(e) => Err(io::Error::new(
                ErrorKind::Other,
                format!("Failed to send stream readable event: {}", e),
            )),
        }
    }

    fn notify_writable_streams(&self, streams: Vec<u64>) {
        for stream_id in streams {
            let r = match self
                .streams
                .get(&stream_id)
                .ok_or_else(|| io::Error::from(ErrorKind::NotFound))
                .and_then(|s| Ok((self.socket.stream_capacity(stream_id)?, s)))
            {
                Ok((cap, _)) if cap < net_utils::MIN_USABLE_QUIC_STREAM_CAPACITY => {
                    self.socket.notify_stream_waiting_writable(stream_id);
                    Ok(())
                }
                Ok((_, stream)) => {
                    match stream.writable_event_tx.try_send(()) {
                        // `Full` is not considered as an error in this case, as the stream does not need
                        // multiple `writable` messages in the queue
                        Ok(_) | Err(mpsc::error::TrySendError::Full(_)) => Ok(()),
                        Err(e) => Err(io::Error::new(ErrorKind::Other, e)),
                    }
                }
                Err(e) => Err(e),
            };

            if let Err(e) = r {
                log_id!(
                    debug,
                    self.parent_id_chain,
                    "Failed to send stream writable event: stream id={} error={}",
                    stream_id,
                    e
                );
            }
        }
    }
}

#[async_trait]
impl HttpCodec for Http3Codec {
    async fn listen(&mut self) -> io::Result<Option<Box<dyn http_codec::Stream>>> {
        enum FiredEvent {
            Stream(StreamMessage),
            Socket(QuicSocketEvent),
        }

        loop {
            let event = {
                let wait_socket_event = self.socket.listen();
                tokio::pin!(wait_socket_event);

                let has_streams = !self.streams.is_empty();
                let wait_stream_message = self.stream_rx.recv();
                tokio::pin!(wait_stream_message);

                tokio::select! {
                    r = wait_socket_event => match r {
                        Ok(event) => Some(FiredEvent::Socket(event)),
                        Err(e) => return Err(e),
                    },
                    message = wait_stream_message, if has_streams => match message {
                        Some(message) => Some(FiredEvent::Stream(message)),
                        None => return Err(io::Error::new(
                            ErrorKind::Other, "Inconsistent state: All stream event senders are closed"
                        )),
                    },
                }
            };

            match event {
                None => (),
                Some(FiredEvent::Stream(message)) => {
                    let stream_id = message.stream_id();
                    if let Err(e) = self.on_stream_message(message) {
                        log_id!(
                            debug,
                            self.parent_id_chain,
                            "Failed to process stream message: id={}, error={}",
                            stream_id,
                            e
                        );
                        let _ = self.on_stream_shutdown(stream_id, None);
                    }
                }
                Some(FiredEvent::Socket(event)) => match self.on_socket_event(event)? {
                    None => (),
                    Some(stream) => return Ok(Some(stream)),
                },
            }
        }
    }

    async fn graceful_shutdown(&mut self) -> io::Result<()> {
        self.socket.graceful_shutdown()
    }

    fn protocol(&self) -> Protocol {
        Protocol::Http3
    }
}

impl StreamMessage {
    fn stream_id(&self) -> u64 {
        match self {
            StreamMessage::WaitingWritable(stream_id) => *stream_id,
            StreamMessage::Shutdown(stream_id, _) => *stream_id,
        }
    }
}

impl http_codec::Stream for DetachedStream {
    fn id(&self) -> log_utils::IdChain<u64> {
        self.source.id.clone()
    }

    fn request(&self) -> &dyn http_codec::PendingRequest {
        &self.source
    }

    fn split(
        self: Box<Self>,
    ) -> (
        Box<dyn http_codec::PendingRequest>,
        Box<dyn http_codec::PendingRespond>,
    ) {
        (Box::new(self.source), Box::new(self.sink))
    }
}

impl http_codec::PendingRequest for StreamSource {
    fn id(&self) -> log_utils::IdChain<u64> {
        self.id.clone()
    }

    fn request(&self) -> &RequestHeaders {
        &self.request
    }

    fn client_address(&self) -> io::Result<IpAddr> {
        self.socket.peer_addr().map(|x| x.ip())
    }

    fn finalize(self: Box<Self>) -> Box<dyn pipe::Source> {
        self
    }
}

#[async_trait]
impl pipe::Source for StreamSource {
    fn id(&self) -> log_utils::IdChain<u64> {
        self.id.clone()
    }

    async fn read(&mut self) -> io::Result<pipe::Data> {
        loop {
            match self.socket.read(self.stream_id)? {
                Some(chunk) => return Ok(pipe::Data::Chunk(chunk)),
                None => {
                    if self.socket.stream_finished(self.stream_id) {
                        return Ok(pipe::Data::Eof);
                    } else {
                        match self.readable_event_rx.recv().await {
                            None => return Err(io::Error::from(ErrorKind::UnexpectedEof)),
                            Some(_) => continue,
                        }
                    }
                }
            }
        }
    }

    fn consume(&mut self, _size: usize) -> io::Result<()> {
        // do nothing as it is consumed by reading from the HTTP3 connection
        Ok(())
    }
}

impl Drop for StreamSource {
    fn drop(&mut self) {
        match self.codec_tx.send(StreamMessage::Shutdown(
            self.stream_id,
            Some(quiche::Shutdown::Read),
        )) {
            Ok(_) => (),
            Err(e) => log_id!(debug, self.id, "Failed to notify of read shutdown: {}", e),
        }
    }
}

impl http_codec::PendingRespond for StreamSink {
    fn id(&self) -> log_utils::IdChain<u64> {
        self.id.clone()
    }

    fn send_response(
        self: Box<Self>,
        response: ResponseHeaders,
        eof: bool,
    ) -> io::Result<Box<dyn http_codec::RespondedStreamSink>> {
        log_id!(
            debug,
            self.id,
            "Sending response: {:?} (eof={})",
            response,
            eof
        );

        self.socket.send_response(self.stream_id, response, false)?;

        if eof {
            self.codec_tx
                .send(StreamMessage::Shutdown(self.stream_id, None))
                .map_err(|e| {
                    io::Error::new(
                        ErrorKind::Other,
                        format!("Failed to send shutdown message: {}", e),
                    )
                })?;
        }

        Ok(self)
    }
}

impl http_codec::RespondedStreamSink for StreamSink {
    fn into_pipe_sink(self: Box<Self>) -> Box<dyn pipe::Sink> {
        self
    }

    fn into_datagram_sink(self: Box<Self>) -> Box<dyn http_codec::DroppingSink> {
        self
    }
}

#[async_trait]
impl pipe::Sink for StreamSink {
    fn id(&self) -> log_utils::IdChain<u64> {
        self.id.clone()
    }

    fn write(&mut self, data: Bytes) -> io::Result<Bytes> {
        let orig_len = data.len();
        let data = self.socket.write(self.stream_id, data)?;

        self.data_frame_overhead = if data.len() == orig_len {
            // Quiche does not shrink the chunk according to stream capacity. Instead, it
            // checks the capacity against `overhead(data.len())` and may return
            // [`quiche::h3::Error::Done`]. As the next `stream_capacity()` call may return
            // a value between `net_utils::quic_data_frame_overhead(1)` and
            // `net_utils::quic_data_frame_overhead(orig_len)` this workaround helps us
            // not fall in a busy loop.
            net_utils::http3_data_frame_overhead(orig_len)
        } else {
            net_utils::http3_data_frame_overhead(1)
        };
        Ok(data)
    }

    fn eof(&mut self) -> io::Result<()> {
        self.socket
            .shutdown_stream(self.stream_id, quiche::Shutdown::Write);
        Ok(())
    }

    async fn wait_writable(&mut self) -> io::Result<()> {
        loop {
            match self.socket.stream_capacity(self.stream_id) {
                Ok(n) if n > self.data_frame_overhead => return Ok(()),
                Ok(_) => {
                    self.codec_tx
                        .send(StreamMessage::WaitingWritable(self.stream_id))
                        .map_err(|_| io::Error::from(ErrorKind::UnexpectedEof))?;
                    match self.writable_event_rx.recv().await {
                        None => return Err(io::Error::from(ErrorKind::UnexpectedEof)),
                        Some(_) => continue,
                    }
                }
                Err(e) => return Err(io::Error::new(ErrorKind::Other, e.to_string())),
            }
        }
    }
}

impl http_codec::DroppingSink for StreamSink {
    fn write(&mut self, data: Bytes) -> io::Result<datagram_pipe::SendStatus> {
        match self.socket.stream_capacity(self.stream_id) {
            Ok(n) if n >= net_utils::http3_data_frame_overhead(data.len()) + data.len() => (),
            Ok(_) => return Ok(datagram_pipe::SendStatus::Dropped),
            Err(e) => return Err(io::Error::new(ErrorKind::Other, e.to_string())),
        }

        let unsent = self.socket.write(self.stream_id, data)?;
        if !unsent.is_empty() {
            return Err(io::Error::new(
                ErrorKind::Other,
                "Packet was sent partially while should've been sent entirely",
            ));
        }
        Ok(datagram_pipe::SendStatus::Sent)
    }
}

impl Drop for StreamSink {
    fn drop(&mut self) {
        match self.codec_tx.send(StreamMessage::Shutdown(
            self.stream_id,
            Some(quiche::Shutdown::Write),
        )) {
            Ok(_) => (),
            Err(e) => log_id!(debug, self.id, "Failed to notify of write shutdown: {}", e),
        }
    }
}
