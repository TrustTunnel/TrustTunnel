use hyper::http::{self, Request};
use std::net::SocketAddr;
use std::time::Duration;
use trusttunnel::net_utils;

#[allow(dead_code)]
mod common;

macro_rules! download_tests {
    ($($name:ident: $client_fn:expr, $body_size_mb:expr,)*) => {
    $(
        #[tokio::test]
        async fn $name() {
            common::set_up_logger();
            let endpoint_address = common::make_endpoint_address();

            let client_task = async {
                tokio::time::sleep(Duration::from_secs(1)).await;
                let (status, body_size) = $client_fn(&endpoint_address, $body_size_mb).await;
                assert_eq!(status, http::StatusCode::OK);
                assert_eq!(body_size, $body_size_mb * 1024 * 1024);
            };

            tokio::select! {
                _ = common::run_endpoint(&endpoint_address) => unreachable!(),
                _ = client_task => (),
                _ = tokio::time::sleep(Duration::from_secs(20)) => panic!("Timed out"),
            }
        }
    )*
    }
}

macro_rules! upload_tests {
    ($($name:ident: $client_fn:expr, $body_size_mb:expr,)*) => {
    $(
        #[tokio::test]
        async fn $name() {
            common::set_up_logger();
            let endpoint_address = common::make_endpoint_address();

            let client_task = async {
                tokio::time::sleep(Duration::from_secs(1)).await;
                let status = $client_fn(&endpoint_address, $body_size_mb * 1024 * 1024).await;
                assert_eq!(status, http::StatusCode::OK);
            };

            tokio::select! {
                _ = common::run_endpoint(&endpoint_address) => unreachable!(),
                _ = client_task => (),
                _ = tokio::time::sleep(Duration::from_secs(20)) => panic!("Timed out"),
            }
        }
    )*
    }
}

download_tests! {
    sni_h1_download: sni_h1_download_client, 3,
    sni_h2_download: sni_h2_download_client, 14,
    sni_h3_download: sni_h3_download_client, 15,
    path_h1_download: path_h1_download_client, 92,
    path_h2_download: path_h2_download_client, 65,
    path_h3_download: path_h3_download_client, 35,
}

upload_tests! {
    sni_h1_upload: sni_h1_upload_client, 89,
    sni_h2_upload: sni_h2_upload_client, 79,
    sni_h3_upload: sni_h3_upload_client, 32,
    path_h1_upload: path_h1_upload_client, 38,
    path_h2_upload: path_h2_upload_client, 46,
    path_h3_upload: path_h3_upload_client, 26,
}

async fn sni_h1_download_client(
    endpoint_address: &SocketAddr,
    body_size_mb: usize,
) -> (http::StatusCode, usize) {
    let stream = common::establish_tls_connection(
        &format!("speed.{}", common::MAIN_DOMAIN_NAME),
        endpoint_address,
        None,
    )
    .await;

    let (response, body) = common::do_get_request(
        stream,
        http::Version::HTTP_11,
        &format!(
            "https://speed.{}:{}/{}mb.bin",
            common::MAIN_DOMAIN_NAME,
            endpoint_address.port(),
            body_size_mb
        ),
        &[],
    )
    .await;

    (response.status, body.len())
}

async fn sni_h2_download_client(
    endpoint_address: &SocketAddr,
    body_size_mb: usize,
) -> (http::StatusCode, usize) {
    let stream = common::establish_tls_connection(
        &format!("speed.{}", common::MAIN_DOMAIN_NAME),
        endpoint_address,
        Some(net_utils::HTTP2_ALPN.as_bytes()),
    )
    .await;

    let (response, body) = common::do_get_request(
        stream,
        http::Version::HTTP_2,
        &format!(
            "https://speed.{}:{}/{}mb.bin",
            common::MAIN_DOMAIN_NAME,
            endpoint_address.port(),
            body_size_mb
        ),
        &[],
    )
    .await;

    (response.status, body.len())
}

async fn sni_h3_download_client(
    endpoint_address: &SocketAddr,
    body_size_mb: usize,
) -> (http::StatusCode, usize) {
    let mut conn = common::Http3Session::connect(
        endpoint_address,
        &format!("speed.{}", common::MAIN_DOMAIN_NAME),
        None,
    )
    .await;

    let (response, content) = conn
        .exchange(
            Request::get(&format!(
                "https://{}:{}/speed/{}mb.bin",
                common::MAIN_DOMAIN_NAME,
                endpoint_address.port(),
                body_size_mb
            ))
            .body(hyper::Body::empty())
            .unwrap(),
        )
        .await;

    (response.status, content.len())
}

async fn path_h1_download_client(
    endpoint_address: &SocketAddr,
    body_size_mb: usize,
) -> (http::StatusCode, usize) {
    let stream =
        common::establish_tls_connection(common::MAIN_DOMAIN_NAME, endpoint_address, None).await;

    let (response, body) = common::do_get_request(
        stream,
        http::Version::HTTP_11,
        &format!(
            "https://{}:{}/speed/{}mb.bin",
            common::MAIN_DOMAIN_NAME,
            endpoint_address.port(),
            body_size_mb
        ),
        &[],
    )
    .await;

    (response.status, body.len())
}

async fn path_h2_download_client(
    endpoint_address: &SocketAddr,
    body_size_mb: usize,
) -> (http::StatusCode, usize) {
    let stream = common::establish_tls_connection(
        common::MAIN_DOMAIN_NAME,
        endpoint_address,
        Some(net_utils::HTTP2_ALPN.as_bytes()),
    )
    .await;

    let (response, body) = common::do_get_request(
        stream,
        http::Version::HTTP_2,
        &format!(
            "https://{}:{}/speed/{}mb.bin",
            common::MAIN_DOMAIN_NAME,
            endpoint_address.port(),
            body_size_mb
        ),
        &[],
    )
    .await;

    (response.status, body.len())
}

async fn path_h3_download_client(
    endpoint_address: &SocketAddr,
    body_size_mb: usize,
) -> (http::StatusCode, usize) {
    let mut conn =
        common::Http3Session::connect(endpoint_address, common::MAIN_DOMAIN_NAME, None).await;

    let (response, content) = conn
        .exchange(
            Request::get(&format!(
                "https://{}:{}/speed/{}mb.bin",
                common::MAIN_DOMAIN_NAME,
                endpoint_address.port(),
                body_size_mb
            ))
            .body(hyper::Body::empty())
            .unwrap(),
        )
        .await;

    (response.status, content.len())
}

async fn sni_h1_upload_client(endpoint_address: &SocketAddr, body_size: usize) -> http::StatusCode {
    let stream = common::establish_tls_connection(
        &format!("speed.{}", common::MAIN_DOMAIN_NAME),
        endpoint_address,
        None,
    )
    .await;

    common::do_post_request(
        stream,
        http::Version::HTTP_11,
        &format!(
            "https://speed.{}:{}/upload.html",
            common::MAIN_DOMAIN_NAME,
            endpoint_address.port()
        ),
        body_size,
    )
    .await
    .status()
}

async fn sni_h2_upload_client(endpoint_address: &SocketAddr, body_size: usize) -> http::StatusCode {
    let stream = common::establish_tls_connection(
        &format!("speed.{}", common::MAIN_DOMAIN_NAME),
        endpoint_address,
        Some(net_utils::HTTP2_ALPN.as_bytes()),
    )
    .await;

    common::do_post_request(
        stream,
        http::Version::HTTP_2,
        &format!(
            "https://speed.{}:{}/upload.html",
            common::MAIN_DOMAIN_NAME,
            endpoint_address.port()
        ),
        body_size,
    )
    .await
    .status()
}

async fn sni_h3_upload_client(endpoint_address: &SocketAddr, body_size: usize) -> http::StatusCode {
    let mut conn = common::Http3Session::connect(
        endpoint_address,
        &format!("speed.{}", common::MAIN_DOMAIN_NAME),
        None,
    )
    .await;
    conn.send_request(
        Request::post(&format!(
            "https://speed.{}:{}/upload.html",
            common::MAIN_DOMAIN_NAME,
            endpoint_address.port()
        ))
        .header(http::header::CONTENT_LENGTH, body_size.to_string())
        .body(hyper::Body::from(vec![0; body_size]))
        .unwrap(),
    )
    .await;

    conn.recv_response().await.status
}

async fn path_h1_upload_client(
    endpoint_address: &SocketAddr,
    body_size: usize,
) -> http::StatusCode {
    let stream =
        common::establish_tls_connection(common::MAIN_DOMAIN_NAME, endpoint_address, None).await;

    common::do_post_request(
        stream,
        http::Version::HTTP_11,
        &format!(
            "https://{}:{}/speed/upload.html",
            common::MAIN_DOMAIN_NAME,
            endpoint_address.port()
        ),
        body_size,
    )
    .await
    .status()
}

async fn path_h2_upload_client(
    endpoint_address: &SocketAddr,
    body_size: usize,
) -> http::StatusCode {
    let stream = common::establish_tls_connection(
        common::MAIN_DOMAIN_NAME,
        endpoint_address,
        Some(net_utils::HTTP2_ALPN.as_bytes()),
    )
    .await;

    common::do_post_request(
        stream,
        http::Version::HTTP_2,
        &format!(
            "https://{}:{}/speed/upload.html",
            common::MAIN_DOMAIN_NAME,
            endpoint_address.port()
        ),
        body_size,
    )
    .await
    .status()
}

async fn path_h3_upload_client(
    endpoint_address: &SocketAddr,
    body_size: usize,
) -> http::StatusCode {
    let mut conn =
        common::Http3Session::connect(endpoint_address, common::MAIN_DOMAIN_NAME, None).await;
    conn.send_request(
        Request::post(&format!(
            "https://{}:{}/speed/upload.html",
            common::MAIN_DOMAIN_NAME,
            endpoint_address.port()
        ))
        .header(http::header::CONTENT_LENGTH, body_size.to_string())
        .body(hyper::Body::from(vec![0; body_size]))
        .unwrap(),
    )
    .await;

    conn.recv_response().await.status
}
