use crate::http_codec::HttpCodec;
use crate::shutdown::Shutdown;
use crate::{log_id, log_utils};
use http::header::{CONTENT_LENGTH, CONTENT_TYPE};
use http::StatusCode;
use std::sync::{Arc, Mutex};
use std::time::Duration;

pub(crate) async fn listen(
    shutdown: Arc<Mutex<Shutdown>>,
    mut codec: Box<dyn HttpCodec>,
    timeout: Duration,
    log_id: log_utils::IdChain<u64>,
) {
    let (mut shutdown_notification, _shutdown_completion) = {
        let shutdown = shutdown.lock().unwrap();
        (shutdown.notification_handler(), shutdown.completion_guard())
    };

    let listen_task = async {
        match codec.listen().await {
            Ok(Some(stream)) => {
                log_id!(
                    trace,
                    log_id,
                    "Deny handler received request: {:?}",
                    stream.request().request()
                );
                let response = http::Response::builder()
                    .status(StatusCode::NOT_FOUND)
                    .header(CONTENT_TYPE, "text/plain; charset=utf-8")
                    .header(CONTENT_LENGTH, "0")
                    .body(())
                    .unwrap()
                    .into_parts()
                    .0;
                if let Err(e) = stream.split().1.send_response(response, true) {
                    log_id!(debug, log_id, "Failed to send deny response: {}", e);
                }
            }
            Ok(None) => log_id!(debug, log_id, "Connection closed before any request"),
            Err(e) => log_id!(debug, log_id, "Session error: {}", e),
        }
    };

    tokio::select! {
        x = shutdown_notification.wait() => {
            match x {
                Ok(_) => (),
                Err(e) => log_id!(debug, log_id, "Shutdown notification failure: {}", e),
            }
        },
        _ = listen_task => (),
        _ = tokio::time::sleep(timeout) => log_id!(debug, log_id, "Session timed out"),
    }

    if let Err(e) = codec.graceful_shutdown().await {
        log_id!(debug, log_id, "Failed to shut down session: {}", e);
    }
}

