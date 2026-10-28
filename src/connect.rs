use crate::client::ClientManager;
use crate::config::local_commit;
use lan_mouse_ipc::{ClientHandle, DEFAULT_PORT};
use lan_mouse_proto::{MAX_EVENT_SIZE, ProtoEvent};
use local_channel::mpsc::{Receiver, Sender, channel};
use std::sync::RwLock;
use std::{
    cell::RefCell,
    collections::{HashMap, HashSet},
    io,
    net::SocketAddr,
    rc::Rc,
    sync::Arc,
    time::Duration,
};
use thiserror::Error;
use tokio::{
    net::UdpSocket,
    sync::Mutex,
    task::{JoinSet, spawn_local},
};
use webrtc_dtls::{
    config::{Config, ExtendedMasterSecretType},
    conn::DTLSConn,
    crypto::Certificate,
};
use webrtc_util::Conn;

#[derive(Debug, Error)]
pub(crate) enum LanMouseConnectionError {
    #[error(transparent)]
    Bind(#[from] io::Error),
    #[error(transparent)]
    Dtls(#[from] webrtc_dtls::Error),
    #[error(transparent)]
    Webrtc(#[from] webrtc_util::Error),
    #[error("not connected")]
    NotConnected,
    #[error("emulation is disabled on the target device")]
    TargetEmulationDisabled,
    #[error("Connection timed out")]
    Timeout,
}

const DEFAULT_CONNECTION_TIMEOUT: Duration = Duration::from_secs(5);

async fn connect(
    addr: SocketAddr,
    cert: Certificate,
) -> Result<(Arc<dyn Conn + Sync + Send>, SocketAddr), (SocketAddr, LanMouseConnectionError)> {
    log::info!("connecting to {addr} ...");
    let conn = Arc::new(
        UdpSocket::bind("0.0.0.0:0")
            .await
            .map_err(|e| (addr, e.into()))?,
    );
    conn.connect(addr).await.map_err(|e| (addr, e.into()))?;
    let config = Config {
        certificates: vec![cert],
        server_name: "ignored".to_owned(),
        insecure_skip_verify: true,
        extended_master_secret: ExtendedMasterSecretType::Require,
        ..Default::default()
    };
    let timeout = tokio::time::sleep(DEFAULT_CONNECTION_TIMEOUT);
    tokio::select! {
        _ = timeout => Err((addr, LanMouseConnectionError::Timeout)),
        result = DTLSConn::new(conn, config, true, None) => match result {
            Ok(dtls_conn) => Ok((Arc::new(dtls_conn), addr)),
            Err(e) => Err((addr, e.into())),
        }
    }
}

async fn connect_any(
    addrs: &[SocketAddr],
    cert: Certificate,
) -> Result<(Arc<dyn Conn + Send + Sync>, SocketAddr), LanMouseConnectionError> {
    let mut joinset = JoinSet::new();
    for &addr in addrs {
        joinset.spawn_local(connect(addr, cert.clone()));
    }
    loop {
        match joinset.join_next().await {
            None => return Err(LanMouseConnectionError::NotConnected),
            Some(r) => match r.expect("join error") {
                Ok(conn) => return Ok(conn),
                Err((a, e)) => {
                    log::warn!("failed to connect to {a}: `{e}`")
                }
            },
        };
    }
}

pub(crate) struct LanMouseConnection {
    cert: Certificate,
    client_manager: ClientManager,
    /// fingerprints trusted for inbound traffic; an outbound connection adds the
    /// peer's fingerprint here so one approval authorizes both directions
    authorized_keys: Arc<RwLock<HashMap<String, String>>>,
    /// fingerprints newly authorized by an outbound connection, for the service
    /// to persist and surface in the UI
    authorized_tx: Sender<String>,
    conns: Rc<Mutex<HashMap<SocketAddr, Arc<dyn Conn + Send + Sync>>>>,
    connecting: Rc<Mutex<HashSet<ClientHandle>>>,
    recv_rx: Receiver<(ClientHandle, ProtoEvent)>,
    recv_tx: Sender<(ClientHandle, ProtoEvent)>,
    ping_response: Rc<RefCell<HashSet<SocketAddr>>>,
}

impl LanMouseConnection {
    pub(crate) fn new(
        cert: Certificate,
        client_manager: ClientManager,
        authorized_keys: Arc<RwLock<HashMap<String, String>>>,
        authorized_tx: Sender<String>,
    ) -> Self {
        let (recv_tx, recv_rx) = channel();
        Self {
            cert,
            client_manager,
            authorized_keys,
            authorized_tx,
            conns: Default::default(),
            connecting: Default::default(),
            recv_rx,
            recv_tx,
            ping_response: Default::default(),
        }
    }

    pub(crate) async fn recv(&mut self) -> (ClientHandle, ProtoEvent) {
        self.recv_rx.recv().await.expect("channel closed")
    }

    /// Send `event`, spawning a background connection attempt if the client is
    /// not connected yet.
    ///
    /// Returns [`LanMouseConnectionError::NotConnected`] when the connection is
    /// still being established: that is not a failure, it means "try again in a
    /// moment".
    pub(crate) async fn send(
        &self,
        event: ProtoEvent,
        handle: ClientHandle,
    ) -> Result<(), LanMouseConnectionError> {
        self.send_inner(event, handle, true).await
    }

    /// Send `event` without starting a new connection attempt.
    ///
    /// Used on release notices: if the connection is already gone there is
    /// nobody to notify, and spawning a fresh connection solely to deliver the
    /// notice would be pointless.
    pub(crate) async fn send_if_connected(
        &self,
        event: ProtoEvent,
        handle: ClientHandle,
    ) -> Result<(), LanMouseConnectionError> {
        self.send_inner(event, handle, false).await
    }

    async fn send_inner(
        &self,
        event: ProtoEvent,
        handle: ClientHandle,
        connect_on_demand: bool,
    ) -> Result<(), LanMouseConnectionError> {
        let (buf, len): ([u8; MAX_EVENT_SIZE], usize) = event.into();
        let buf = &buf[..len];
        if let Some(addr) = self.client_manager.active_addr(handle) {
            let conn = {
                let conns = self.conns.lock().await;
                conns.get(&addr).cloned()
            };
            if let Some(conn) = conn {
                if !self.client_manager.alive(handle) {
                    return Err(LanMouseConnectionError::TargetEmulationDisabled);
                }
                match conn.send(buf).await {
                    Ok(_) => {}
                    Err(e) => {
                        log::warn!("client {handle} failed to send: {e}");
                        disconnect(&self.client_manager, handle, addr, &self.conns).await;
                    }
                }
                log::trace!("{event} >->->->->- {addr}");
                return Ok(());
            }
        }

        if !connect_on_demand {
            return Err(LanMouseConnectionError::NotConnected);
        }

        // check if we are already trying to connect
        let mut connecting = self.connecting.lock().await;
        if !connecting.contains(&handle) {
            connecting.insert(handle);
            // connect in the background
            spawn_local(connect_to_handle(
                self.client_manager.clone(),
                self.cert.clone(),
                handle,
                Shared {
                    authorized_keys: self.authorized_keys.clone(),
                    authorized_tx: self.authorized_tx.clone(),
                    conns: self.conns.clone(),
                    connecting: self.connecting.clone(),
                    tx: self.recv_tx.clone(),
                    ping_response: self.ping_response.clone(),
                },
            ));
        }
        Err(LanMouseConnectionError::NotConnected)
    }
}

/// State shared with every spawned connection task.
#[derive(Clone)]
struct Shared {
    authorized_keys: Arc<RwLock<HashMap<String, String>>>,
    authorized_tx: Sender<String>,
    conns: Rc<Mutex<HashMap<SocketAddr, Arc<dyn Conn + Send + Sync>>>>,
    connecting: Rc<Mutex<HashSet<ClientHandle>>>,
    tx: Sender<(ClientHandle, ProtoEvent)>,
    ping_response: Rc<RefCell<HashSet<SocketAddr>>>,
}

async fn connect_to_handle(
    client_manager: ClientManager,
    cert: Certificate,
    handle: ClientHandle,
    shared: Shared,
) -> Result<(), LanMouseConnectionError> {
    let Shared {
        authorized_keys,
        authorized_tx,
        conns,
        connecting,
        tx,
        ping_response,
    } = shared;
    log::info!("client {handle} connecting ...");
    // sending did not work, figure out active conn.
    if let Some(addrs) = client_manager.get_ips(handle) {
        let port = client_manager.get_port(handle).unwrap_or(DEFAULT_PORT);
        let addrs = addrs
            .into_iter()
            .map(|a| SocketAddr::new(a, port))
            .collect::<Vec<_>>();
        log::info!("client ({handle}) connecting ... (ips: {addrs:?})");
        let res = connect_any(&addrs, cert).await;
        let (conn, addr) = match res {
            Ok(c) => c,
            Err(e) => {
                connecting.lock().await.remove(&handle);
                return Err(e);
            }
        };
        log::info!("client ({handle}) connected @ {addr}");

        // Authorize the peer we just reached, so its inbound connections are
        // accepted without a second manual approval on this machine. The
        // fingerprint is read from the completed handshake, never from the
        // `insecure_skip_verify` bypass, so it is the peer's real identity.
        authorize_peer(&conn, &authorized_keys, &authorized_tx).await;

        client_manager.set_active_addr(handle, Some(addr));
        conns.lock().await.insert(addr, conn.clone());
        connecting.lock().await.remove(&handle);

        // Best-effort version handshake. Send our commit hash once
        // immediately after the DTLS handshake; the listen side
        // mirrors a Hello back so the receive loop can populate
        // `peer_commit`. Old peers will silently skip this event
        // per the forward-compat handler in [`receive_loop`].
        let (buf, len) = ProtoEvent::Hello {
            commit: local_commit(),
        }
        .into();
        if let Err(e) = conn.send(&buf[..len]).await {
            log::debug!("hello send to {addr} failed: {e}");
        }

        // poll connection for active
        spawn_local(ping_pong(addr, conn.clone(), ping_response.clone()));

        // receiver
        spawn_local(receive_loop(
            client_manager,
            handle,
            addr,
            conn,
            conns,
            tx,
            ping_response.clone(),
        ));
        return Ok(());
    }
    connecting.lock().await.remove(&handle);
    Err(LanMouseConnectionError::NotConnected)
}

/// Record the connected peer's certificate fingerprint as trusted.
///
/// This is what makes one approval cover both directions: the operator chose
/// this client, so the machine that answered is the peer we expect. The
/// fingerprint comes from the completed handshake, so it is the peer's real
/// identity even though the outbound leg does not pin it.
async fn authorize_peer(
    conn: &Arc<dyn Conn + Send + Sync>,
    authorized_keys: &Arc<RwLock<HashMap<String, String>>>,
    authorized_tx: &Sender<String>,
) {
    use webrtc_dtls::conn::DTLSConn;

    let Some(dtls_conn) = conn.as_any().downcast_ref::<DTLSConn>() else {
        log::debug!("connected peer is not a DTLS connection, not authorizing");
        return;
    };
    let Some(cert) = dtls_conn
        .connection_state()
        .await
        .peer_certificates
        .first()
        .cloned()
    else {
        log::debug!("connected peer presented no certificate, not authorizing");
        return;
    };

    let fingerprint = crate::crypto::generate_fingerprint(&cert);
    let inserted = authorized_keys
        .write()
        .expect("lock")
        .insert(fingerprint.clone(), "outgoing peer".to_owned())
        .is_none();
    if inserted {
        log::info!("authorized outgoing peer {fingerprint}");
        // let the service persist it and show it in the UI
        if let Err(e) = authorized_tx.send(fingerprint) {
            log::warn!("could not report authorized peer: {e}");
        }
    }
}

async fn ping_pong(
    addr: SocketAddr,
    conn: Arc<dyn Conn + Send + Sync>,
    ping_response: Rc<RefCell<HashSet<SocketAddr>>>,
) {
    loop {
        let (buf, len) = ProtoEvent::Ping.into();

        // send 4 pings, at least one must be answered
        for _ in 0..4 {
            if let Err(e) = conn.send(&buf[..len]).await {
                log::warn!("{addr}: send error `{e}`, closing connection");
                let _ = conn.close().await;
                break;
            }
            log::trace!("PING >->->->->- {addr}");

            tokio::time::sleep(Duration::from_millis(500)).await;
        }

        if !ping_response.borrow_mut().remove(&addr) {
            log::warn!("{addr} did not respond, closing connection");
            let _ = conn.close().await;
            return;
        }
    }
}

async fn receive_loop(
    client_manager: ClientManager,
    handle: ClientHandle,
    addr: SocketAddr,
    conn: Arc<dyn Conn + Send + Sync>,
    conns: Rc<Mutex<HashMap<SocketAddr, Arc<dyn Conn + Send + Sync>>>>,
    tx: Sender<(ClientHandle, ProtoEvent)>,
    ping_response: Rc<RefCell<HashSet<SocketAddr>>>,
) {
    let mut buf = [0u8; MAX_EVENT_SIZE];
    while conn.recv(&mut buf).await.is_ok() {
        match buf.try_into() {
            Ok(event) => {
                log::trace!("{addr} <==<==<== {event}");
                match event {
                    ProtoEvent::Pong(b) => {
                        client_manager.set_active_addr(handle, Some(addr));
                        client_manager.set_alive(handle, b);
                        ping_response.borrow_mut().insert(addr);
                    }
                    ProtoEvent::Hello { commit } => {
                        client_manager.set_peer_commit(handle, Some(commit));
                    }
                    event => tx.send((handle, event)).expect("channel closed"),
                }
            }
            // Skip undecodable datagrams without dropping the
            // connection. Each DTLS recv is one framed message, so
            // skipping is safe and keeps us forward-compatible with
            // peers that send event types we don't yet know about.
            Err(e) => log::debug!("ignoring undecodable event from {addr}: {e}"),
        }
    }
    log::warn!("recv error");
    disconnect(&client_manager, handle, addr, &conns).await;
}

async fn disconnect(
    client_manager: &ClientManager,
    handle: ClientHandle,
    addr: SocketAddr,
    conns: &Mutex<HashMap<SocketAddr, Arc<dyn Conn + Send + Sync>>>,
) {
    log::warn!("client ({handle}) @ {addr} connection closed");
    conns.lock().await.remove(&addr);
    client_manager.set_active_addr(handle, None);
    client_manager.set_peer_commit(handle, None);
    let active: Vec<SocketAddr> = conns.lock().await.keys().copied().collect();
    log::info!("active connections: {active:?}");
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::ClientManager;
    use crate::config::ConfigClient;
    use lan_mouse_ipc::Position;
    use std::net::IpAddr;
    use std::sync::RwLock;
    use webrtc_util::conn::Listener;

    /// One approval must authorize both directions: connecting out to a peer
    /// records that peer's fingerprint, so its inbound connections to this
    /// machine are accepted without a second approval.
    ///
    /// Uses a real DTLS handshake over loopback.
    #[test]
    fn outbound_connection_authorizes_the_peer_for_inbound() {
        // unique directory under the OS temp dir; no extra dependency needed
        let dir = std::env::temp_dir().join(format!(
            "lan-mouse-test-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let our_cert =
            crate::crypto::load_or_generate_key_and_cert(&dir.join("a.pem")).expect("our cert");
        let peer_cert =
            crate::crypto::load_or_generate_key_and_cert(&dir.join("b.pem")).expect("peer cert");
        let peer_fingerprint = crate::crypto::certificate_fingerprint(&peer_cert);

        // the peer listens; we know nothing about it yet
        let authorized: Arc<RwLock<HashMap<String, String>>> = Default::default();
        assert!(
            authorized.read().unwrap().is_empty(),
            "precondition: nothing authorized yet"
        );

        let port = {
            let l = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
            l.local_addr().unwrap().port()
        };

        let (tx, _rx) = local_channel::mpsc::channel();
        let manager = ClientManager::default();
        let handle = manager.add_with_config(ConfigClient {
            ips: HashSet::from([IpAddr::from([127, 0, 0, 1])]),
            hostname: None,
            port,
            pos: Position::Left,
            active: true,
            enter_hook: None,
            leave_hook: None,
        });

        // `connect_any` performs the DTLS handshake, so a listener must be up.
        let peer_cert_clone = peer_cert.clone();
        let (server_ready_tx, server_ready_rx) = std::sync::mpsc::channel();
        let server = std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_io()
                .enable_time()
                .build()
                .unwrap();
            rt.block_on(async move {
                let cfg = webrtc_dtls::config::Config {
                    certificates: vec![peer_cert_clone],
                    extended_master_secret: webrtc_dtls::config::ExtendedMasterSecretType::Require,
                    client_auth: webrtc_dtls::config::ClientAuthType::RequireAnyClientCert,
                    ..Default::default()
                };
                let listener =
                    webrtc_dtls::listener::listen(SocketAddr::from(([127, 0, 0, 1], port)), cfg)
                        .await
                        .expect("listen");
                server_ready_tx.send(()).unwrap();
                // accept one connection and hold it open
                let (conn, _addr) = listener.accept().await.expect("accept");
                tokio::time::sleep(Duration::from_secs(3)).await;
                let _ = conn.close().await;
            });
        });

        server_ready_rx
            .recv_timeout(Duration::from_secs(10))
            .expect("peer listener did not start");

        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_io()
            .enable_time()
            .build()
            .unwrap();
        // connect_to_handle spawns local tasks, so it needs a LocalSet
        let local = tokio::task::LocalSet::new();
        local
            .block_on(
                &rt,
                connect_to_handle(
                    manager,
                    our_cert,
                    handle,
                    Shared {
                        authorized_keys: authorized.clone(),
                        authorized_tx: tx,
                        conns: Default::default(),
                        connecting: Default::default(),
                        tx: {
                            let (t, _r) = local_channel::mpsc::channel();
                            t
                        },
                        ping_response: Default::default(),
                    },
                ),
            )
            .expect("outbound connect should succeed");

        let keys = authorized.read().unwrap().clone();
        assert!(
            keys.contains_key(&peer_fingerprint),
            "the peer's fingerprint must be recorded so the reverse direction is authorized; got {keys:?}"
        );
        server.join().ok();
    }
}
