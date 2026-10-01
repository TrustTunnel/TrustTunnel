use crate::http_codec::HttpCodec;
use crate::{core, http_codec, log_id, log_utils, subscription};
use base64::engine::general_purpose::STANDARD as BASE64_ENGINE;
use base64::Engine;
use bytes::Bytes;
use http::StatusCode;
use std::io;
use std::io::ErrorKind;
use std::sync::Arc;

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
        log_id!(debug, log_id, "Failed to shut down HTTP session: {}", e);
    }
}

async fn listen_inner(
    context: Arc<core::Context>,
    codec: &mut dyn HttpCodec,
    sni: String,
    log_id: &log_utils::IdChain<u64>,
) {
    loop {
        match codec.listen().await {
            Ok(Some(stream)) => {
                let id = stream.id();
                if let Err(e) =
                    handle_stream(context.clone(), stream, sni.clone(), id.clone()).await
                {
                    log_id!(debug, id, "Subscription request failed: {}", e);
                }
            }
            Ok(None) => {
                log_id!(trace, log_id, "Connection closed");
                break;
            }
            Err(ref e) if e.kind() == ErrorKind::UnexpectedEof => {
                log_id!(trace, log_id, "Connection closed");
                break;
            }
            Err(e) => {
                log_id!(debug, log_id, "Session error: {}", e);
                break;
            }
        }
    }
}

async fn handle_stream(
    context: Arc<core::Context>,
    stream: Box<dyn http_codec::Stream>,
    sni: String,
    log_id: log_utils::IdChain<u64>,
) -> io::Result<()> {
    let (request, respond) = stream.split();
    let req = request.request();
    let method = req.method.clone();
    log_id!(
        trace,
        log_id,
        "Received subscription request: {} {}",
        method,
        req.uri
    );

    // Precedence: SNI mismatch -> 404, bad creds -> 401, disabled -> 403,
    // non-GET -> 405, then 200. SNI is checked before auth so the endpoint never
    // serves subscription data on the wrong TLS host.
    let config = context.subscription.read().unwrap().clone();
    let Some(config) = config else {
        respond.send_bad_response(StatusCode::NOT_FOUND, vec![])?;
        return Ok(());
    };

    if sni != config.hostname {
        respond.send_bad_response(StatusCode::NOT_FOUND, vec![])?;
        return Ok(());
    }

    let Some(client) = authenticate(&context, &req.headers) else {
        respond.send_bad_response(
            StatusCode::UNAUTHORIZED,
            vec![(
                "www-authenticate".to_string(),
                "Basic realm=\"Subscription\"".to_string(),
            )],
        )?;
        return Ok(());
    };

    if !config.enabled {
        respond.send_bad_response(StatusCode::FORBIDDEN, vec![])?;
        return Ok(());
    }

    if method != http::Method::GET {
        respond.send_bad_response(
            StatusCode::METHOD_NOT_ALLOWED,
            vec![("allow".to_string(), "GET".to_string())],
        )?;
        return Ok(());
    }

    let response = subscription::SubscriptionResponse {
        version: 1,
        hostname: &config.hostname,
        address: &config.address,
        username: &client.0,
        password: &client.1,
        has_ipv6: true,
        upstream_protocol: "http2",
        anti_dpi: false,
        skip_verification: false,
        certificate: config.certificate.as_deref(),
        custom_sni: config.custom_sni.as_deref(),
        client_random_prefix: config.client_random_prefix.as_deref(),
        name: config.name.as_deref(),
        dns_upstreams: &config.dns_upstreams,
    };

    let body =
        serde_json::to_vec(&response).map_err(|e| io::Error::other(format!("JSON encode: {e}")))?;
    let response_headers = http::Response::builder()
        .status(StatusCode::OK)
        .header(http::header::CONTENT_TYPE, "application/json")
        .header(http::header::CONTENT_LENGTH, body.len().to_string())
        .body(())
        .unwrap()
        .into_parts()
        .0;

    let mut sink = respond
        .send_response(response_headers, false)?
        .into_pipe_sink();
    sink.write_all(Bytes::from(body)).await?;
    sink.eof()?;
    Ok(())
}

/// Decode `Authorization: Basic <b64>` and match it against the credential registry.
fn authenticate(context: &core::Context, headers: &http::HeaderMap) -> Option<(String, String)> {
    let header = headers.get(http::header::AUTHORIZATION)?.to_str().ok()?;
    let encoded = header.strip_prefix("Basic ")?.trim();
    let decoded = BASE64_ENGINE.decode(encoded).ok()?;
    let creds = String::from_utf8(decoded).ok()?;
    let (user, pass) = creds.split_once(':')?;
    context
        .settings
        .clients
        .iter()
        .find(|c| c.username == user && c.password == pass)
        .map(|c| (c.username.clone(), c.password.clone()))
}
