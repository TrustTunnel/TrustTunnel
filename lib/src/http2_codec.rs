use std::io;
use std::io::ErrorKind;
use std::net::IpAddr;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use async_trait::async_trait;
use bytes::Bytes;
use h2::{Reason, RecvStream, SendStream, server};
use h2::server::{Connection, Handshake, SendResponse};
use tokio::io::{AsyncRead, AsyncWrite};
use crate::http_codec::{HttpCodec, RequestHeaders, ResponseHeaders};
use crate::{datagram_pipe, http_codec, log_id, log_utils, net_utils, pipe};
use crate::tls_demultiplexer::Protocol;
use crate::settings::{ListenProtocolSettings, Settings};


pub(crate) struct Http2Codec<IO> {
    state: State<IO>,
    parent_id_chain: log_utils::IdChain<u64>,
    next_conn_id: std::ops::RangeFrom<u64>,
    client_address: IpAddr,
}

enum State<IO> {
    Handshake(Handshake<IO>),
    Established(Connection<IO, Bytes>),
}


struct Stream {
    request: Request,
    respond: Respond,
}

struct Request {
    request: RequestHeaders,
    rx: RecvStream,
    client_address: IpAddr,
    id: log_utils::IdChain<u64>,
}

struct RequestStream {
    rx: RecvStream,
    id: log_utils::IdChain<u64>,
}

struct Respond {
    tx: SendResponse<Bytes>,
    id: log_utils::IdChain<u64>,
}

struct RespondStream {
    tx: SendStream<Bytes>,
    id: log_utils::IdChain<u64>,
}


impl<IO> Http2Codec<IO>
    where IO: AsyncRead + AsyncWrite + Unpin + net_utils::PeerAddr
{
    pub fn new(
        core_settings: Arc<Settings>,
        transport_stream: IO,
        parent_id_chain: log_utils::IdChain<u64>,
    ) -> io::Result<Self> {
        let http2_settings = core_settings.listen_protocols.iter()
            .find(|x| matches!(x, ListenProtocolSettings::Http2(_)))
            .map(|x| match x {
                ListenProtocolSettings::Http2(x) => x,
                _ => unreachable!(),
            })
            .unwrap();

        let client_address = transport_stream.peer_addr()?.ip();

        Ok(Self {
            state: State::Handshake(
                server::Builder::new()
                    .initial_connection_window_size(http2_settings.initial_connection_window_size)
                    .initial_window_size(http2_settings.initial_stream_window_size)
                    .max_concurrent_streams(http2_settings.max_concurrent_streams)
                    .max_frame_size(http2_settings.max_frame_size)
                    .max_header_list_size(http2_settings.header_table_size)
                    .handshake(transport_stream)
            ),
            parent_id_chain,
            next_conn_id: 0..,
            client_address,
        })
    }
}

#[async_trait]
impl<IO: AsyncRead + AsyncWrite + Send + Unpin> HttpCodec for Http2Codec<IO> {
    async fn listen(&mut self) -> io::Result<Option<Box<dyn http_codec::Stream>>> {
        if let State::Handshake(handshake) = &mut self.state {
            self.state = State::Established(handshake.await.map_err(h2_to_io_error)?);
            log_id!(trace, self.parent_id_chain, "HTTP2 connection established");
        }

        let session = match &mut self.state {
            State::Handshake(_) => unreachable!(),
            State::Established(s) => s,
        };

        match session.accept().await {
            Some(Ok((request, respond))) => {
                let (request, rx) = request.into_parts();
                let id = self.parent_id_chain.extended(log_utils::IdItem::new(
                    log_utils::CONNECTION_ID_FMT, self.next_conn_id.next().unwrap(),
                ));
                // @note: [`h2::StreamId`] cannot be converted to raw integer, so just log it
                //        to have a link between stream and out own generated IDs in the logs
                // @note: could be worked around by allowing any id type in the id chain
                log_id!(debug, id, "H2 stream id: {:?}", rx.stream_id());
                Ok(Some(Box::new(Stream {
                    request: Request {
                        request,
                        rx,
                        client_address: self.client_address,
                        id: id.clone(),
                    },
                    respond: Respond {
                        tx: respond,
                        id,
                    },
                })))
            }
            Some(Err(e)) if e.is_io() => Err(e.into_io().unwrap()),
            Some(Err(e)) if e.reason() == Some(Reason::NO_ERROR) => Ok(None),
            Some(Err(e)) => Err(h2_to_io_error(e)),
            None => Ok(None),
        }
    }

    async fn graceful_shutdown(&mut self) -> io::Result<()> {
        let session = match &mut self.state {
            State::Handshake(_) => return Ok(()),
            State::Established(s) => s,
        };

        session.graceful_shutdown();

        loop {
            match session.accept().await {
                None => break Ok(()),
                Some(Err(e)) if e.is_io() => break Err(e.into_io().unwrap()),
                Some(_) => continue,
            }
        }
    }

    fn protocol(&self) -> Protocol {
        Protocol::Http2
    }
}

impl http_codec::Stream for Stream {
    fn id(&self) -> log_utils::IdChain<u64> {
        self.request.id.clone()
    }

    fn request(&self) -> &dyn http_codec::PendingRequest {
        &self.request
    }

    fn split(self: Box<Self>) -> (Box<dyn http_codec::PendingRequest>, Box<dyn http_codec::PendingRespond>) {
        (Box::new(self.request), Box::new(self.respond))
    }
}

impl http_codec::PendingRequest for Request {
    fn id(&self) -> log_utils::IdChain<u64> {
        self.id.clone()
    }

    fn request(&self) -> &RequestHeaders {
        &self.request
    }

    fn client_address(&self) -> io::Result<IpAddr> {
        Ok(self.client_address)
    }

    fn finalize(self: Box<Self>) -> Box<dyn pipe::Source> {
        Box::new(RequestStream {
            rx: self.rx,
            id: self.id,
        })
    }
}

#[async_trait]
impl pipe::Source for RequestStream {
    fn id(&self) -> log_utils::IdChain<u64> {
        self.id.clone()
    }

    async fn read(&mut self) -> io::Result<pipe::Data> {
        match self.rx.data().await {
            None => Ok(pipe::Data::Eof),
            Some(Ok(chunk)) => Ok(pipe::Data::Chunk(chunk)),
            Some(Err(e)) if e.reason().map_or(true, |r| r == Reason::NO_ERROR) =>
                Ok(pipe::Data::Eof),
            Some(Err(e)) => Err(h2_to_io_error(e)),
        }
    }

    fn consume(&mut self, size: usize) -> io::Result<()> {
        self.rx.flow_control().release_capacity(size).map_err(h2_to_io_error)
    }
}

impl http_codec::PendingRespond for Respond {
    fn id(&self) -> log_utils::IdChain<u64> {
        self.id.clone()
    }

    fn send_response(mut self: Box<Self>, response: ResponseHeaders, eof: bool)
                     -> io::Result<Box<dyn http_codec::RespondedStreamSink>>
    {
        log_id!(debug, self.id, "Sending response: {:?} (eof={})", response, eof);

        let tx = self.tx.send_response(http::Response::from_parts(response, ()), eof)
            .map_err(h2_to_io_error)?;

        Ok(Box::new(RespondStream {
            tx,
            id: self.id,
        }))
    }
}

impl http_codec::RespondedStreamSink for RespondStream {
    fn into_pipe_sink(self: Box<Self>) -> Box<dyn pipe::Sink> {
        self
    }

    fn into_datagram_sink(self: Box<Self>) -> Box<dyn http_codec::DroppingSink> {
        self
    }
}

pub struct WaitWritable<'a> {
    stream: &'a mut SendStream<Bytes>,
}

impl<'a> std::future::Future for WaitWritable<'a> {
    type Output = io::Result<()>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        if self.stream.capacity() > 0 {
            return Poll::Ready(Ok(()));
        }

        Poll::Ready(
            match futures::ready!(self.stream.poll_capacity(cx)) {
                None => Err(io::Error::from(ErrorKind::UnexpectedEof)),
                Some(Ok(_)) => Ok(()),
                Some(Err(e)) => Err(h2_to_io_error(e)),
            }
        )
    }
}

#[async_trait]
impl pipe::Sink for RespondStream {
    fn id(&self) -> log_utils::IdChain<u64> {
        self.id.clone()
    }

    fn write(&mut self, mut data: Bytes) -> io::Result<Bytes> {
        self.tx.reserve_capacity(data.len());
        self.tx.send_data(data.split_to(self.tx.capacity()), false).map_err(h2_to_io_error)?;
        Ok(data)
    }

    fn eof(&mut self) -> io::Result<()> {
        self.tx.send_data(Bytes::new(), true).map_err(h2_to_io_error)
    }

    async fn wait_writable(&mut self) -> io::Result<()> {
        WaitWritable { stream: &mut self.tx }.await
    }
}

impl http_codec::DroppingSink for RespondStream {
    fn write(&mut self, data: Bytes) -> io::Result<datagram_pipe::SendStatus> {
        self.tx.reserve_capacity(data.len());

        if self.tx.capacity() >= data.len() {
            self.tx.send_data(data, false)
                .map(|_| datagram_pipe::SendStatus::Sent)
                .map_err(h2_to_io_error)
        } else {
            Ok(datagram_pipe::SendStatus::Dropped)
        }
    }
}

fn h2_to_io_error(e: h2::Error) -> io::Error {
    let reason = e.reason();
    if reason.as_ref().map_or(true, |r| *r == Reason::NO_ERROR) {
        return io::Error::from(ErrorKind::UnexpectedEof);
    }

    e.into_io()
        .unwrap_or_else(|| io::Error::new(ErrorKind::Other, format!("HTTP2 error: {:?}", reason)))
}
