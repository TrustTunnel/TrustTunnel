use crate::tls_demultiplexer::Protocol;
use crate::{authentication, datagram_pipe, log_utils, pipe};
use async_trait::async_trait;
use bytes::Bytes;
use http::uri::Authority;
use http::{Response, StatusCode};
use std::io;
use std::io::ErrorKind;
use std::net::IpAddr;

pub(crate) type RequestHeaders = http::request::Parts;
pub(crate) type ResponseHeaders = http::response::Parts;

/// Encapsulates an HTTP stream implementation
pub(crate) trait Stream: Send {
    /// Get the request ID for logging
    fn id(&self) -> log_utils::IdChain<u64>;

    /// Get the request part
    fn request(&self) -> &dyn PendingRequest;

    /// Split the stream into the receiving and transmitting parts
    fn split(self: Box<Self>) -> (Box<dyn PendingRequest>, Box<dyn PendingRespond>);
}

/// Encapsulates a receiving part of an HTTP stream state
pub(crate) trait PendingRequest: Send {
    /// Get the request ID for logging
    fn id(&self) -> log_utils::IdChain<u64>;

    /// Get the request headers
    fn request(&self) -> &RequestHeaders;

    /// Clone the request headers of the stream
    fn clone_request(&self) -> RequestHeaders {
        let request = self.request();
        let mut builder = http::Request::builder()
            .method(request.method.clone())
            .uri(request.uri.clone())
            .version(request.version);
        if let Some(hs) = builder.headers_mut() {
            *hs = request.headers.clone();
        }
        // assume it's ok as original headers were built successfully
        builder.body(()).unwrap().into_parts().0
    }

    /// Get the address of a VPN client made the connection request
    fn client_address(&self) -> io::Result<IpAddr>;

    /// Get the authorization info if some
    fn auth_info(&self) -> io::Result<Option<authentication::Source>> {
        let headers = &self.request().headers;
        let mut basic_auth: Option<&str> = None;
        let mut bearer_auth: Option<&str> = None;

        for header_name in [
            http::header::AUTHORIZATION,
            http::header::PROXY_AUTHORIZATION,
        ] {
            let Some(raw_header) = headers.get(header_name) else {
                continue;
            };

            let header = raw_header.to_str().map_err(|_| {
                io::Error::new(
                    ErrorKind::Other,
                    format!("Unexpected authorization header: {:?}", self.request()),
                )
            })?;

            if bearer_auth.is_none() {
                bearer_auth = header.strip_prefix("Bearer ");
            }
            if basic_auth.is_none() {
                basic_auth = header.strip_prefix("Basic ");
            }
        }

        Ok(match (bearer_auth, basic_auth) {
            (Some(bearer), Some(basic)) => Some(authentication::Source::ProxyBearerAndBasic {
                bearer: bearer.into(),
                basic: basic.into(),
            }),
            (Some(bearer), None) => Some(authentication::Source::ProxyBearer(bearer.into())),
            (None, Some(basic)) => Some(authentication::Source::ProxyBasic(basic.into())),
            (None, None) => None,
        })
    }

    /// Get the request authority
    fn authority(&self) -> io::Result<&Authority> {
        self.request().uri.authority().ok_or_else(|| {
            io::Error::new(
                ErrorKind::Other,
                format!("Authority not found: {:?}", self.request()),
            )
        })
    }

    /// Get the user agent
    fn user_agent(&self) -> Option<String> {
        self.request()
            .headers
            .get(http::header::USER_AGENT)
            .map(http::header::HeaderValue::to_str)?
            .ok()
            .map(String::from)
    }

    /// Turn the pending request into the [`pipe::Source`] object
    fn finalize(self: Box<Self>) -> Box<dyn pipe::Source>;
}

/// Encapsulates a non-responded transmitting part of an HTTP stream state
pub(crate) trait PendingRespond: Send {
    /// Get the request ID for logging
    #[allow(dead_code)]
    fn id(&self) -> log_utils::IdChain<u64>;

    /// Send the intermediate response to a client. Unlike `send_response()`,
    /// it does not change the pending state of the object.
    fn send_intermediate_response(&self, _: ResponseHeaders) -> io::Result<()> {
        Ok(())
    }

    /// Send the response to a client
    fn send_response(
        self: Box<Self>,
        response: ResponseHeaders,
        eof: bool,
    ) -> io::Result<Box<dyn RespondedStreamSink>>;

    /// Send the OK response to a client
    fn send_ok_response(self: Box<Self>, eof: bool) -> io::Result<Box<dyn RespondedStreamSink>> {
        self.send_response(Response::<()>::default().into_parts().0, eof)
    }

    /// Send a bad response to a client
    fn send_bad_response(
        self: Box<Self>,
        status: StatusCode,
        extra_headers: Vec<(String, String)>,
    ) -> io::Result<()> {
        let response = {
            let mut b = Response::builder().status(status);
            for (n, v) in extra_headers {
                b = b.header(n, v);
            }
            b.body(()).unwrap()
        };

        self.send_response(response.into_parts().0, true)
            .map(|_| ())
    }
}

/// An abstract interface for a datagram transmitter implementation
pub(crate) trait DroppingSink: Send {
    fn write(&mut self, data: Bytes) -> io::Result<datagram_pipe::SendStatus>;
}

/// A helper trait which converts a stream sink wrapper into one of the sink types
pub(crate) trait RespondedStreamSink: Send {
    fn into_pipe_sink(self: Box<Self>) -> Box<dyn pipe::Sink>;
    fn into_datagram_sink(self: Box<Self>) -> Box<dyn DroppingSink>;
}

/// An abstract interface for an HTTP server-side session implementation
#[async_trait]
pub(crate) trait HttpCodec: Send {
    /// Listen to IO events.
    /// Returns `None` in case the listening finished gracefully and should not be continued,
    /// `Some` in case a client initiated the new stream which should be processed and listening
    /// should be continued.
    async fn listen(&mut self) -> io::Result<Option<Box<dyn Stream>>>;

    /// Shut down the HTTP session gracefully
    async fn graceful_shutdown(&mut self) -> io::Result<()>;

    /// Get the codec protocol
    fn protocol(&self) -> Protocol;
}

/// Turn a [`Stream`] into a [`HttpCodec`] which produces the single stream.
pub(crate) fn stream_into_codec(stream: Box<dyn Stream>, protocol: Protocol) -> impl HttpCodec {
    SingleRequestCodec {
        stream: Some(stream),
        protocol,
    }
}

struct SingleRequestCodec {
    stream: Option<Box<dyn Stream>>,
    protocol: Protocol,
}

#[async_trait]
impl HttpCodec for SingleRequestCodec {
    async fn listen(&mut self) -> io::Result<Option<Box<dyn Stream>>> {
        if let Some(stream) = self.stream.take() {
            return Ok(Some(stream));
        }

        futures::future::pending().await
    }

    async fn graceful_shutdown(&mut self) -> io::Result<()> {
        if let Some((_, respond)) = self.stream.take().map(Stream::split) {
            let _ = respond.send_bad_response(http::StatusCode::INTERNAL_SERVER_ERROR, vec![]);
        }

        Ok(())
    }

    fn protocol(&self) -> Protocol {
        self.protocol
    }
}
