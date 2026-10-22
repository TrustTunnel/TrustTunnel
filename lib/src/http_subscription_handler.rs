use crate::http_codec::HttpCodec;
use crate::{core, http_codec, log_id, log_utils, subscription};
use base64::engine::general_purpose::STANDARD as BASE64_ENGINE;
use base64::Engine;
use bytes::Bytes;
use http::StatusCode;
use std::io;
use std::io::ErrorKind;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

#[derive(Default)]
struct SessionManager {
    active_requests: AtomicUsize,
}

/// Serve subscription requests. `sni` is `Some` on the main-host path, where the
/// request SNI must match the configured hostname, and `None` on a dedicated
/// host, where the TLS layer already matched it.
pub(crate) async fn listen(
    context: Arc<core::Context>,
    mut codec: Box<dyn HttpCodec>,
    sni: Option<String>,
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
    sni: Option<String>,
    log_id: &log_utils::IdChain<u64>,
) {
    let manager = Arc::new(SessionManager::default());
    let timeout = context.settings.tls_handshake_timeout;
    loop {
        match tokio::time::timeout(timeout, codec.listen()).await {
            Ok(Ok(Some(stream))) => {
                let id = stream.id();
                tokio::spawn({
                    let context = context.clone();
                    let manager = manager.clone();
                    let sni = sni.clone();
                    async move {
                        manager.active_requests.fetch_add(1, Ordering::AcqRel);
                        if let Err(e) = handle_stream(context, stream, sni, &id).await {
                            log_id!(debug, id, "Subscription request failed: {}", e);
                        }
                        manager.active_requests.fetch_sub(1, Ordering::AcqRel);
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
            Err(_elapsed) if manager.active_requests.load(Ordering::Acquire) > 0 => log_id!(
                trace,
                log_id,
                "Ignoring timeout due to there are some active requests"
            ),
            Err(_elapsed) => {
                log_id!(debug, log_id, "Closing due to timeout");
                if let Err(e) = codec.graceful_shutdown().await {
                    log_id!(debug, log_id, "Failed to shutdown HTTP session: {}", e);
                }
                break;
            }
        }
    }
}

async fn handle_stream(
    context: Arc<core::Context>,
    stream: Box<dyn http_codec::Stream>,
    sni: Option<String>,
    log_id: &log_utils::IdChain<u64>,
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

    // Precedence: missing config or wrong host/path -> 404, bad creds -> 401,
    // disabled -> 403, non-GET -> 405, then 200. The host/path check is done
    // before auth so subscription data is never served on the wrong TLS host.
    let config = context.subscription.read().unwrap().clone();
    let Some(config) = config else {
        respond.send_bad_response(StatusCode::NOT_FOUND, Default::default())?;
        return Ok(());
    };

    if !is_subscription_request(sni.as_deref(), req.uri.path(), &config) {
        respond.send_bad_response(StatusCode::NOT_FOUND, Default::default())?;
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
        respond.send_bad_response(StatusCode::FORBIDDEN, Default::default())?;
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
        client_random_auth_key: config.client_random_auth_key.as_deref(),
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

fn is_subscription_request(
    sni: Option<&str>,
    path: &str,
    config: &subscription::SubscriptionConfig,
) -> bool {
    let host_matches = sni.is_none_or(|sni| sni == config.hostname.as_str());
    host_matches && path == config.path
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

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> subscription::SubscriptionConfig {
        subscription::SubscriptionConfig {
            enabled: true,
            path: "/subscription".to_string(),
            hostname: "vpn.example.com".to_string(),
            address: "203.0.113.1:443".to_string(),
            name: None,
            dns_upstreams: vec![],
            certificate: None,
            custom_sni: None,
            client_random_prefix: None,
            client_random_auth_key: None,
        }
    }

    #[test]
    fn dedicated_host_accepts_exact_path() {
        assert!(is_subscription_request(None, "/subscription", &config()));
    }

    #[test]
    fn dedicated_host_rejects_other_path() {
        assert!(!is_subscription_request(None, "/other", &config()));
    }

    #[test]
    fn path_route_rejects_foreign_sni() {
        assert!(!is_subscription_request(
            Some("other.example.com"),
            "/subscription",
            &config(),
        ));
    }

    #[test]
    fn path_route_rejects_other_path() {
        assert!(!is_subscription_request(
            Some("vpn.example.com"),
            "/other",
            &config(),
        ));
    }

    #[test]
    fn path_route_accepts_matching_sni_and_path() {
        assert!(is_subscription_request(
            Some("vpn.example.com"),
            "/subscription",
            &config(),
        ));
    }
}
