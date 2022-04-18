use std::io;
use std::io::ErrorKind;
use std::sync::{Arc, Mutex};
use tokio::time;
use crate::authorization::Status;
use crate::downstream::{AuthorizedRequest, Downstream, PendingDatagramMultiplexerRequest, PendingTcpConnectRequest};
use crate::forwarder::Forwarder;
use crate::{datagram_pipe, downstream, log_id, log_utils, pipe, udp_pipe};
use crate::pipe::{DuplexPipe, SimplexPipe, SimplexPipeDirection};
use crate::settings::Settings;


pub(crate) struct Tunnel {
    core_settings: Arc<Settings>,
    downstream: Box<dyn Downstream>,
    forwarder: Arc<Mutex<Box<dyn Forwarder>>>,
    id: log_utils::IdChain<u64>,
}


impl Tunnel {
    pub fn new(
        core_settings: Arc<Settings>,
        downstream: Box<dyn Downstream>,
        forwarder: Box<dyn Forwarder>,
        id: log_utils::IdChain<u64>,
    ) -> Self {
        Self {
            core_settings,
            downstream,
            forwarder: Arc::new(Mutex::new(forwarder)),
            id,
        }
    }

    pub async fn listen(&mut self) -> io::Result<()> {
        loop {
            let request = match tokio::time::timeout(
                self.core_settings.client_listener_timeout, self.downstream.listen()
            ).await {
                Ok(Ok(None)) => {
                    log_id!(debug, self.id, "Tunnel closed gracefully");
                    return Ok(());
                }
                Ok(Ok(Some(r))) => r,
                Ok(Err(e)) => return Err(e),
                Err(_) => return Err(io::Error::from(ErrorKind::TimedOut)),
            };

            let core_settings = self.core_settings.clone();
            let forwarder = self.forwarder.clone();
            let request_id = request.id();

            tokio::spawn(async move {
                let info = match request.auth_info() {
                    Ok(x) => x,
                    Err(e) => {
                        log_id!(debug, request_id, "Failed to get auth info: {}", e);
                        request.fail_request();
                        return;
                    }
                };

                match core_settings.authorizer.authorize(info).await {
                    Status::Pass => (),
                    Status::Reject => {
                        log_id!(debug, request_id, "Authorization failed");
                        request.fail_request();
                        return;
                    }
                }

                match request.succeed_request() {
                    Ok(None) => (),
                    Ok(Some(AuthorizedRequest::TcpConnect(request))) => {
                        Tunnel::on_tcp_connect_request(core_settings, forwarder, request).await
                    }
                    Ok(Some(AuthorizedRequest::DatagramMultiplexer(request))) => {
                        Tunnel::on_datagram_mux_request(core_settings, forwarder, request).await
                    }
                    Err(e) => {
                        log_id!(debug, request_id, "Failed to complete request: {}", e);
                    }
                }
            });
        }
    }

    async fn on_tcp_connect_request(
        core_settings: Arc<Settings>,
        forwarder: Arc<Mutex<Box<dyn Forwarder>>>,
        request: Box<dyn PendingTcpConnectRequest>,
    ) {
        let request_id = request.id();
        let destination = match request.destination() {
            Ok(d) => d,
            Err(e) => {
                log_id!(debug, request_id, "Failed to get destination: {}", e);
                let _ = request.fail_request(e);
                return;
            }
        };

        log_id!(trace, request_id, "Connecting to peer: {:?}", destination);
        let connector =
            match forwarder.lock().unwrap().tcp_connector(request_id.clone(), destination) {
                Ok(c) => c,
                Err(e) => {
                    log_id!(debug, request_id, "Failed to start connection: {}", e);
                    let _ = request.fail_request(e);
                    return;
                }
            };

        let (fwd_rx, fwd_tx) =
            match time::timeout(core_settings.tcp_connections_timeout, connector.connect()).await
                .unwrap_or_else(|_| Err(io::Error::from(ErrorKind::TimedOut)))
            {
                Ok(x) => x,
                Err(e) => {
                    log_id!(debug, request_id, "Connection to peer failed: {}", e);
                    let _ = request.fail_request(e);
                    return;
                }
            };

        log_id!(trace, request_id, "Successfully connected to peer");
        let (dstr_rx, dstr_tx) =
            match request.succeed_request() {
                Ok(x) => x,
                Err(e) => {
                    log_id!(debug, request_id, "Failed to complete request: {}", e);
                    return;
                }
            };

        let mut pipe = DuplexPipe::new(
            SimplexPipe::new(dstr_rx, fwd_tx, SimplexPipeDirection::Outgoing),
            SimplexPipe::new(fwd_rx, dstr_tx, SimplexPipeDirection::Incoming),
        );

        match pipe.exchange(core_settings.tcp_connections_timeout).await {
            Ok(_) => { log_id!(trace, request_id, "Both ends closed gracefully"); }
            Err(e) => { log_id!(debug, request_id, "Error on pipe: {}", e); }
        }
    }

    async fn on_datagram_mux_request(
        core_settings: Arc<Settings>,
        forwarder: Arc<Mutex<Box<dyn Forwarder>>>,
        request: Box<dyn PendingDatagramMultiplexerRequest>,
    ) {
        let request_id = request.id();
        let mut pipe: Box<dyn datagram_pipe::DuplexPipe> = match request.succeed_request() {
            Ok(downstream::DatagramPipeHalves::Udp(dstr_source, dstr_sink)) => {
                let (fwd_shared, fwd_source, fwd_sink) =
                    match forwarder.lock().unwrap().make_udp_datagram_multiplexer(request_id.clone()) {
                        Ok(x) => x,
                        Err(e) => {
                            log_id!(debug, request_id, "Failed to create datagram multiplexer: {}", e);
                            return;
                        }
                    };

                Box::new(udp_pipe::DuplexPipe::new(
                    (dstr_source, dstr_sink),
                    (fwd_shared, fwd_source, fwd_sink),
                    core_settings.udp_connections_timeout,
                ))
            }
            Ok(downstream::DatagramPipeHalves::Icmp(dstr_source, dstr_sink)) => {
                let (fwd_source, fwd_sink) =
                    match forwarder.lock().unwrap().make_icmp_datagram_multiplexer(request_id.clone()) {
                        Ok(x) => x,
                        Err(e) => {
                            log_id!(debug, request_id, "Failed to create datagram multiplexer: {}", e);
                            return;
                        }
                    };

                Box::new(datagram_pipe::GenericDuplexPipe::new(
                    datagram_pipe::GenericSimplexPipe::new(
                        pipe::SimplexPipeDirection::Outgoing,
                        dstr_source,
                        fwd_sink,
                    ),
                    datagram_pipe::GenericSimplexPipe::new(
                        pipe::SimplexPipeDirection::Incoming,
                        fwd_source,
                        dstr_sink,
                    ),
                ))
            }
            Err(e) => {
                log_id!(debug, request_id, "Failed to respond for datagram multiplexer request: {}", e);
                return;
            }
        };

        match pipe.exchange().await {
            Ok(_) => log_id!(trace, request_id, "Datagram multiplexer gracefully closed"),
            Err(e) => log_id!(debug, request_id, "Datagram multiplexer closed with error: {}", e),
        }
    }
}
