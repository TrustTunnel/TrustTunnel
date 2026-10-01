use crate::{http_codec, net_utils, settings, subscription, tls_demultiplexer};
use std::sync::{Arc, RwLock};

pub(crate) struct HttpDemux {
    core_settings: Arc<settings::Settings>,
    subscription: Arc<RwLock<Option<subscription::SubscriptionConfig>>>,
}

impl HttpDemux {
    pub fn new(
        core_settings: Arc<settings::Settings>,
        subscription: Arc<RwLock<Option<subscription::SubscriptionConfig>>>,
    ) -> Self {
        Self {
            core_settings,
            subscription,
        }
    }

    pub fn select(
        &self,
        _protocol: tls_demultiplexer::Protocol,
        request: &http_codec::RequestHeaders,
    ) -> net_utils::Channel {
        match () {
            _ if self.check_subscription(request) => net_utils::Channel::Subscription,
            _ if self.check_ping(request) => net_utils::Channel::Ping,
            _ if self.check_speedtest(request) => net_utils::Channel::Speedtest,
            _ if self.check_reverse_proxy_path(request) => net_utils::Channel::ReverseProxy,
            _ => net_utils::Channel::Tunnel,
        }
    }

    fn check_subscription(&self, request: &http_codec::RequestHeaders) -> bool {
        let sub = self.subscription.read().unwrap();
        match sub.as_ref() {
            Some(cfg) => request.uri.path() == cfg.path,
            None => false,
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
}
