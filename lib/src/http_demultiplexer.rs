use crate::{authentication, http_codec, net_utils, settings, tls_demultiplexer};
use std::sync::Arc;

pub(crate) struct HttpDemux {
    core_settings: Arc<settings::Settings>,
}

impl HttpDemux {
    pub fn new(core_settings: Arc<settings::Settings>) -> Self {
        Self { core_settings }
    }

    pub fn select(
        &self,
        protocol: tls_demultiplexer::Protocol,
        request: &http_codec::RequestHeaders,
    ) -> net_utils::Channel {
        let tunnel_token = self.tunnel_token_header(request);
        match () {
            _ if self.check_ping(request) => net_utils::Channel::Ping,
            _ if self.check_speedtest(request) => net_utils::Channel::Speedtest,
            _ if tunnel_token.is_some_and(|t| self.check_tunnel_token_value(t)) => {
                net_utils::Channel::Tunnel
            }
            _ if tunnel_token.is_some() => self.fallback_channel(protocol),
            _ if self.check_legacy_tunnel(request) => net_utils::Channel::Tunnel,
            _ if self.check_reverse_proxy_path(request) => net_utils::Channel::ReverseProxy,
            _ => self.fallback_channel(protocol),
        }
    }

    fn check_ping(&self, request: &http_codec::RequestHeaders) -> bool {
        if !self.core_settings.ping_enable {
            return false;
        }
        if let Some(path) = self.core_settings.ping_path.as_ref() {
            return request.uri.path().starts_with(path);
        }
        false
    }

    fn check_speedtest(&self, request: &http_codec::RequestHeaders) -> bool {
        if !self.core_settings.speedtest_enable {
            return false;
        }
        if let Some(path) = self.core_settings.speedtest_path.as_ref() {
            return request.uri.path().starts_with(path);
        }
        false
    }

    fn tunnel_token_header<'a>(
        &self,
        request: &'a http_codec::RequestHeaders,
    ) -> Option<&'a str> {
        request
            .headers
            .get(http::HeaderName::from_static("x-tunnel-token"))
            .and_then(|x| x.to_str().ok())
    }

    fn check_tunnel_token_value(&self, header: &str) -> bool {
        self.core_settings.clients.iter().any(|client| {
            let expected =
                authentication::tunnel_token_from_credentials(&client.username, &client.password);
            expected == header
        })
    }

    fn check_legacy_tunnel(&self, request: &http_codec::RequestHeaders) -> bool {
        if !self.core_settings.allow_without_token {
            return false;
        }
        request.method == http::Method::CONNECT
            || request
                .headers
                .contains_key(http::header::PROXY_AUTHORIZATION)
    }

    fn check_reverse_proxy_path(&self, request: &http_codec::RequestHeaders) -> bool {
        if self.core_settings.reverse_proxy.is_none() {
            return false;
        }
        self.core_settings
            .reverse_proxy
            .as_ref()
            .map(|x| x.path_mask.as_str())
            .is_some_and(|x| request.uri.path().starts_with(x))
    }

    fn fallback_channel(&self, protocol: tls_demultiplexer::Protocol) -> net_utils::Channel {
        let _ = protocol;
        net_utils::Channel::Deny
    }
}
