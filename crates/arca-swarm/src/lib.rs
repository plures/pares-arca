//! Pares Arca Swarm — Hyperswarm-style P2P cache replication.
//!
//! # Overview
//!
//! `arca-swarm` turns a local-only Pares Arca binary cache into a
//! distributed one by enabling peer-to-peer synchronisation of narinfo
//! metadata.
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────┐
//! │                        SwarmNode                            │
//! │                                                             │
//! │  ┌──────────────┐    UDP multicast     ┌──────────────┐    │
//! │  │  Discovery   │◄───── Announce ──────│  Discovery   │    │
//! │  │  (UDP 7070)  │──── AnnounceReply ──►│  (UDP 7070)  │    │
//! │  └──────┬───────┘                      └──────┬───────┘    │
//! │         │                                     │            │
//! │         │  HolePunch (UDP)                    │            │
//! │         │◄───────────────────────────────────►│            │
//! │         │                                     │            │
//! │  ┌──────▼───────┐  Noise XX (TCP 7071) ┌──────▼───────┐   │
//! │  │  Sync server │◄──── encrypted ──────│  Sync client │   │
//! │  │              │─── HaveList/Want/Data►│              │   │
//! │  └──────────────┘                      └──────────────┘   │
//! │                                                             │
//! │  ┌──────────────┐                                          │
//! │  │  NarInfoCrdt │  LWW-Map CRDT — merged with peer data   │
//! │  └──────────────┘                                          │
//! └─────────────────────────────────────────────────────────────┘
//! ```
//!
//! # Peer discovery
//!
//! Nodes periodically broadcast a `DiscoveryMsg::Announce` datagram on the
//! `239.255.255.250:7070` multicast group.  The announce contains a
//! hex-encoded SHA-256 topic hash derived from the shared topic string
//! (so the raw topic is never sent on the wire).  Peers that recognise the
//! hash reply with `DiscoveryMsg::AnnounceReply`.
//!
//! # Encrypted transport
//!
//! After discovery, peers connect over TCP using the
//! **Noise_XX_25519_ChaChaPoly_BLAKE2s** pattern, which provides mutual
//! authentication and forward secrecy.
//!
//! # CRDT sync
//!
//! After the Noise handshake each side sends its [`SyncMsg::HaveList`] of
//! `(hash, timestamp)` pairs.  The remote replies with
//! [`SyncMsg::WantList`] for entries it is missing or has an older version
//! of.  Requested entries are sent as [`SyncMsg::NarInfoData`] messages and
//! written to disk.
//!
//! # Graceful fallback
//!
//! If no peers reply within `DISCOVERY_TIMEOUT` (5 s) the node continues
//! operating as a local-only cache and periodically re-announces in case
//! peers come online later.

pub mod crdt;
pub mod discovery;
pub mod error;
pub mod protocol;
pub mod topic;
pub mod transport;

use std::collections::HashSet;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{broadcast, Mutex};
use tracing::{debug, info, warn};

use crdt::{CrdtEntry, NarInfoCrdt};
use discovery::{DiscoverySocket, ANNOUNCE_INTERVAL, DISCOVERY_TIMEOUT, RESYNC_INTERVAL};
use error::SwarmError;
use protocol::{DiscoveryMsg, HaveEntry, SyncMsg};
use topic::{derive_topic_hash, topic_hash_hex};
use transport::{
    generate_ephemeral_keypair, load_or_generate_keypair, noise_handshake_initiator,
    noise_handshake_responder, recv_encrypted, send_encrypted, Keypair,
};

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// Configuration for a [`SwarmNode`].
#[derive(Debug, Clone)]
pub struct SwarmConfig {
    /// Shared topic name — all nodes with the same topic sync together.
    ///
    /// The raw string is never sent over the network; only its SHA-256
    /// hash is used for peer identification.
    pub topic: String,

    /// UDP port used for discovery announcements and hole-punch probes.
    pub discovery_port: u16,

    /// TCP port on which the Noise-encrypted sync server listens.
    pub sync_port: u16,

    /// HTTP port of the co-located `arca-server` (announced to peers so
    /// they know where to fetch NAR archives).
    pub http_port: u16,

    /// Optional static peers to connect to unconditionally (e.g. bootstrap
    /// nodes or peers behind symmetric NAT that can't be reached by multicast).
    pub static_peers: Vec<SocketAddr>,

    /// When `true`, generate an ephemeral (in-memory) keypair instead of
    /// persisting one in the cache directory.  Useful for tests.
    pub ephemeral_keys: bool,
}

impl Default for SwarmConfig {
    fn default() -> Self {
        Self {
            topic: "pares-arca-default".into(),
            discovery_port: 7070,
            sync_port: 7071,
            http_port: 5555,
            static_peers: vec![],
            ephemeral_keys: false,
        }
    }
}

/// Events emitted by the swarm during operation.
#[derive(Debug, Clone)]
pub enum SwarmEvent {
    /// A new peer was discovered and successfully synced.
    PeerSynced {
        peer_addr: SocketAddr,
        new_paths: usize,
    },
    /// A peer was found but the sync connection failed.
    PeerSyncFailed {
        peer_addr: SocketAddr,
        reason: String,
    },
    /// No peers replied to the initial discovery broadcast.
    NoPeers,
}

// ---------------------------------------------------------------------------
// Discovered peer state
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Hash, PartialEq, Eq)]
struct PeerKey {
    /// Hex-encoded Noise public key (unique peer identifier).
    noise_pubkey: String,
}

#[derive(Debug, Clone)]
struct DiscoveredPeer {
    /// Source address of the peer's discovery datagram.
    discovery_addr: SocketAddr,
    /// TCP port on which the peer accepts sync connections.
    sync_port: u16,
    /// HTTP server port of the peer.
    http_port: u16,
    /// Noise public key (hex).
    noise_pubkey: String,
}

impl DiscoveredPeer {
    fn sync_addr(&self) -> SocketAddr {
        SocketAddr::new(self.discovery_addr.ip(), self.sync_port)
    }

    fn http_base_url(&self) -> String {
        format!("http://{}:{}", self.discovery_addr.ip(), self.http_port)
    }
}

// ---------------------------------------------------------------------------
// SwarmNode
// ---------------------------------------------------------------------------

/// Hyperswarm-inspired P2P sync node.
///
/// Manages peer discovery (UDP multicast), Noise-encrypted sync connections
/// (TCP), and narinfo CRDT state.
pub struct SwarmNode {
    config: SwarmConfig,
    topic_hash: [u8; 32],
    keypair: Arc<Keypair>,
    crdt: Arc<Mutex<NarInfoCrdt>>,
    cache_dir: PathBuf,
}

impl SwarmNode {
    /// Create a new `SwarmNode`.
    ///
    /// Seeds the in-memory CRDT from any `.narinfo` files already present
    /// in `cache_dir`.
    pub async fn new(cache_dir: PathBuf, config: SwarmConfig) -> Result<Self, SwarmError> {
        let topic_hash = derive_topic_hash(&config.topic);

        let keypair = if config.ephemeral_keys {
            Arc::new(generate_ephemeral_keypair()?)
        } else {
            Arc::new(load_or_generate_keypair(&cache_dir)?)
        };

        let crdt = Arc::new(Mutex::new(NarInfoCrdt::new()));
        {
            let mut c = crdt.lock().await;
            if let Err(e) = c.seed_from_dir(&cache_dir) {
                warn!("Could not seed CRDT from cache dir: {e}");
            }
        }

        info!(
            topic = %config.topic,
            topic_hash = %topic_hash_hex(&config.topic),
            pubkey = %keypair.public_hex,
            "SwarmNode initialised"
        );

        Ok(Self {
            config,
            topic_hash,
            keypair,
            crdt,
            cache_dir,
        })
    }

    /// Return the hex-encoded Noise public key for this node.
    pub fn public_key_hex(&self) -> &str {
        &self.keypair.public_hex
    }

    /// Run the swarm until a shutdown signal is received.
    ///
    /// Starts three concurrent tasks:
    /// 1. **Discovery** — periodic UDP multicast announcements and reply
    ///    handling.
    /// 2. **Sync server** — TCP listener that accepts inbound Noise sync
    ///    connections from peers.
    /// 3. **Peer connector** — initiates outbound sync connections to newly
    ///    discovered peers.
    ///
    /// If no peers reply within [`DISCOVERY_TIMEOUT`], logs a warning and
    /// continues in local-only mode (peers can still connect later).
    pub async fn run(
        &self,
        mut shutdown: broadcast::Receiver<()>,
        event_tx: Option<tokio::sync::mpsc::UnboundedSender<SwarmEvent>>,
    ) -> Result<(), SwarmError> {
        // Shared set of peers we have already successfully synced with in
        // this session, to avoid redundant reconnects.
        let synced: Arc<Mutex<HashSet<PeerKey>>> = Arc::new(Mutex::new(HashSet::new()));

        // Channel through which discovery feeds peer info to the connector.
        let (peer_tx, peer_rx) = tokio::sync::mpsc::unbounded_channel::<DiscoveredPeer>();

        // ---- Start the TCP sync server ----
        let sync_listener = TcpListener::bind(format!("0.0.0.0:{}", self.config.sync_port))
            .await
            .map_err(|e| {
                SwarmError::Io(std::io::Error::new(
                    e.kind(),
                    format!("bind sync port {}: {e}", self.config.sync_port),
                ))
            })?;
        info!(port = self.config.sync_port, "Sync server listening");

        let server_crdt = Arc::clone(&self.crdt);
        let server_kp = Arc::clone(&self.keypair);
        let server_cache_dir = self.cache_dir.clone();
        let server_http_port = self.config.http_port;
        let mut server_shutdown = shutdown.resubscribe();

        tokio::spawn(async move {
            loop {
                tokio::select! {
                    accept = sync_listener.accept() => {
                        match accept {
                            Ok((stream, peer_addr)) => {
                                debug!(%peer_addr, "Inbound sync connection");
                                let crdt = Arc::clone(&server_crdt);
                                let kp = Arc::clone(&server_kp);
                                let cache_dir = server_cache_dir.clone();
                                tokio::spawn(async move {
                                    if let Err(e) = handle_inbound_sync(
                                        stream, peer_addr, kp, crdt, cache_dir, server_http_port,
                                    ).await {
                                        warn!(%peer_addr, "Inbound sync error: {e}");
                                    }
                                });
                            }
                            Err(e) => warn!("Accept error: {e}"),
                        }
                    }
                    _ = server_shutdown.recv() => break,
                }
            }
        });

        // ---- Start the peer connector ----
        let connector_crdt = Arc::clone(&self.crdt);
        let connector_kp = Arc::clone(&self.keypair);
        let connector_cache_dir = self.cache_dir.clone();
        let connector_http_port = self.config.http_port;
        let connector_synced = Arc::clone(&synced);
        let connector_event_tx = event_tx.clone();
        let mut connector_shutdown = shutdown.resubscribe();

        tokio::spawn(async move {
            connect_peers(
                peer_rx,
                connector_kp,
                connector_crdt,
                connector_cache_dir,
                connector_http_port,
                connector_synced,
                connector_event_tx,
                &mut connector_shutdown,
            )
            .await;
        });

        // ---- Discovery loop ----
        self.run_discovery(peer_tx, &synced, &event_tx, &mut shutdown)
            .await
    }

    // -----------------------------------------------------------------------
    // Discovery
    // -----------------------------------------------------------------------

    async fn run_discovery(
        &self,
        peer_tx: tokio::sync::mpsc::UnboundedSender<DiscoveredPeer>,
        synced: &Arc<Mutex<HashSet<PeerKey>>>,
        event_tx: &Option<tokio::sync::mpsc::UnboundedSender<SwarmEvent>>,
        shutdown: &mut broadcast::Receiver<()>,
    ) -> Result<(), SwarmError> {
        let sock = DiscoverySocket::bind(self.config.discovery_port).await?;

        info!(
            port = self.config.discovery_port,
            topic = %self.config.topic,
            "Discovery socket bound"
        );

        let announce = self.build_announce();
        let topic_hex = topic_hash_hex(&self.config.topic);

        // ---- Initial static peers ----
        for &addr in &self.config.static_peers {
            let _ = sock
                .send_to(
                    &DiscoveryMsg::AnnounceReply {
                        noise_pubkey: self.keypair.public_hex.clone(),
                        sync_port: self.config.sync_port,
                        http_port: self.config.http_port,
                    },
                    addr,
                )
                .await;
        }

        // ---- Announce immediately and then on a timer ----
        if let Err(e) = sock.multicast(&announce).await {
            warn!("Initial announce failed: {e}");
        }

        // Wait for the first reply within DISCOVERY_TIMEOUT.
        let mut found_peer = false;
        let deadline = tokio::time::Instant::now() + DISCOVERY_TIMEOUT;

        loop {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if !found_peer && remaining.is_zero() {
                info!("No peers found after discovery timeout — local-only mode");
                if let Some(tx) = event_tx {
                    let _ = tx.send(SwarmEvent::NoPeers);
                }
                // Don't return; keep announcing so late-joining peers can connect.
                break;
            }

            let recv_fut = sock.recv();
            let timeout_fut = tokio::time::sleep(remaining.min(Duration::from_millis(200)));

            tokio::select! {
                _ = shutdown.recv() => return Ok(()),
                _ = timeout_fut, if !found_peer => {
                    // let the outer loop re-evaluate the deadline
                }
                result = recv_fut => {
                    match result {
                        Ok((msg, from)) => {
                            self.handle_discovery_msg(
                                msg, from, &sock, &topic_hex, &peer_tx, synced
                            ).await;
                            found_peer = true;
                        }
                        Err(e) => warn!("Discovery recv error: {e}"),
                    }
                }
            }

            if found_peer {
                break;
            }
        }

        // ---- Periodic re-announce + re-sync loop ----
        let mut announce_interval = tokio::time::interval(ANNOUNCE_INTERVAL);
        announce_interval.tick().await; // skip the first immediate tick
        let mut resync_interval = tokio::time::interval(RESYNC_INTERVAL);
        resync_interval.tick().await; // skip the first immediate tick

        loop {
            tokio::select! {
                _ = shutdown.recv() => return Ok(()),
                _ = resync_interval.tick() => {
                    // Clear the synced set so all known peers re-sync their
                    // HaveLists on the next announce cycle. This ensures new
                    // paths built since last sync are propagated.
                    let cleared = {
                        let mut guard = synced.lock().await;
                        let n = guard.len();
                        guard.clear();
                        n
                    };
                    if cleared > 0 {
                        debug!(cleared, "Cleared synced set for periodic re-sync");
                    }
                    // Re-announce immediately to trigger new connections.
                    if let Err(e) = sock.multicast(&announce).await {
                        warn!("Re-sync announce failed: {e}");
                    }
                    // Also re-contact static peers directly.
                    for &addr in &self.config.static_peers {
                        let _ = sock.send_to(
                            &DiscoveryMsg::AnnounceReply {
                                noise_pubkey: self.keypair.public_hex.clone(),
                                sync_port: self.config.sync_port,
                                http_port: self.config.http_port,
                            },
                            addr,
                        ).await;
                    }
                }
                _ = announce_interval.tick() => {
                    if let Err(e) = sock.multicast(&announce).await {
                        warn!("Periodic announce failed: {e}");
                    }
                }
                result = sock.recv() => {
                    match result {
                        Ok((msg, from)) => {
                            self.handle_discovery_msg(
                                msg, from, &sock, &topic_hex, &peer_tx, synced
                            ).await;
                        }
                        Err(e) => warn!("Discovery recv error: {e}"),
                    }
                }
            }
        }
    }

    async fn handle_discovery_msg(
        &self,
        msg: DiscoveryMsg,
        from: SocketAddr,
        sock: &DiscoverySocket,
        topic_hex: &str,
        peer_tx: &tokio::sync::mpsc::UnboundedSender<DiscoveredPeer>,
        synced: &Arc<Mutex<HashSet<PeerKey>>>,
    ) {
        match msg {
            DiscoveryMsg::Announce {
                topic_hash,
                noise_pubkey,
                sync_port,
                http_port,
            } if topic_hash == topic_hex => {
                debug!(%from, "Received matching Announce");
                // Don't reply to ourselves.
                if noise_pubkey == self.keypair.public_hex {
                    return;
                }
                // Reply.
                let reply = DiscoveryMsg::AnnounceReply {
                    noise_pubkey: self.keypair.public_hex.clone(),
                    sync_port: self.config.sync_port,
                    http_port: self.config.http_port,
                };
                if let Err(e) = sock.send_to(&reply, from).await {
                    warn!("AnnounceReply failed: {e}");
                }
                // Queue peer for outbound sync.
                self.queue_peer_if_new(
                    DiscoveredPeer {
                        discovery_addr: from,
                        sync_port,
                        http_port,
                        noise_pubkey,
                    },
                    peer_tx,
                    synced,
                )
                .await;
                // Send hole-punch probe to help with NAT traversal.
                let _ = sock.hole_punch(from, None).await;
            }

            DiscoveryMsg::Announce { topic_hash, .. } => {
                debug!(%from, %topic_hash, "Ignoring Announce for different topic");
            }

            DiscoveryMsg::AnnounceReply {
                noise_pubkey,
                sync_port,
                http_port,
            } => {
                debug!(%from, "Received AnnounceReply");
                if noise_pubkey == self.keypair.public_hex {
                    return;
                }
                self.queue_peer_if_new(
                    DiscoveredPeer {
                        discovery_addr: from,
                        sync_port,
                        http_port,
                        noise_pubkey,
                    },
                    peer_tx,
                    synced,
                )
                .await;
                let _ = sock.hole_punch(from, None).await;
            }

            DiscoveryMsg::HolePunch { our_addr } => {
                debug!(%from, ?our_addr, "Received HolePunch");
                let _ = sock.send_to(&DiscoveryMsg::HolePunchAck, from).await;
            }

            DiscoveryMsg::HolePunchAck => {
                debug!(%from, "Received HolePunchAck");
            }
        }
    }

    async fn queue_peer_if_new(
        &self,
        peer: DiscoveredPeer,
        peer_tx: &tokio::sync::mpsc::UnboundedSender<DiscoveredPeer>,
        synced: &Arc<Mutex<HashSet<PeerKey>>>,
    ) {
        let key = PeerKey {
            noise_pubkey: peer.noise_pubkey.clone(),
        };
        let mut guard = synced.lock().await;
        if guard.insert(key) {
            let _ = peer_tx.send(peer);
        }
    }

    fn build_announce(&self) -> DiscoveryMsg {
        DiscoveryMsg::Announce {
            topic_hash: hex::encode(self.topic_hash),
            noise_pubkey: self.keypair.public_hex.clone(),
            sync_port: self.config.sync_port,
            http_port: self.config.http_port,
        }
    }
}

// ---------------------------------------------------------------------------
// Outbound peer connector task
// ---------------------------------------------------------------------------

#[allow(clippy::too_many_arguments)]
async fn connect_peers(
    mut peer_rx: tokio::sync::mpsc::UnboundedReceiver<DiscoveredPeer>,
    keypair: Arc<Keypair>,
    crdt: Arc<Mutex<NarInfoCrdt>>,
    cache_dir: PathBuf,
    http_port: u16,
    synced: Arc<Mutex<HashSet<PeerKey>>>,
    event_tx: Option<tokio::sync::mpsc::UnboundedSender<SwarmEvent>>,
    shutdown: &mut broadcast::Receiver<()>,
) {
    loop {
        tokio::select! {
            _ = shutdown.recv() => break,
            peer = peer_rx.recv() => {
                let Some(peer) = peer else { break };
                let sync_addr = peer.sync_addr();
                let http_base = peer.http_base_url();
                let kp = Arc::clone(&keypair);
                let crdt2 = Arc::clone(&crdt);
                let cache = cache_dir.clone();
                let ev = event_tx.clone();
                let synced2 = Arc::clone(&synced);
                let peer_key = PeerKey { noise_pubkey: peer.noise_pubkey.clone() };

                tokio::spawn(async move {
                    match initiate_sync(sync_addr, kp, crdt2, cache, http_port, &http_base).await {
                        Ok(new_paths) => {
                            info!(%sync_addr, new_paths, "Sync complete");
                            if let Some(tx) = ev {
                                let _ = tx.send(SwarmEvent::PeerSynced { peer_addr: sync_addr, new_paths });
                            }
                        }
                        Err(e) => {
                            warn!(%sync_addr, "Sync failed: {e}");
                            // Remove from synced set so we can retry later.
                            synced2.lock().await.remove(&peer_key);
                            if let Some(tx) = ev {
                                let _ = tx.send(SwarmEvent::PeerSyncFailed {
                                    peer_addr: sync_addr,
                                    reason: e.to_string(),
                                });
                            }
                        }
                    }
                });
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Outbound sync (initiator side)
// ---------------------------------------------------------------------------

async fn initiate_sync(
    peer_addr: SocketAddr,
    keypair: Arc<Keypair>,
    crdt: Arc<Mutex<NarInfoCrdt>>,
    cache_dir: PathBuf,
    our_http_port: u16,
    peer_http_base: &str,
) -> Result<usize, SwarmError> {
    debug!(%peer_addr, "Connecting for sync");
    let mut stream = TcpStream::connect(peer_addr).await?;
    let mut noise = noise_handshake_initiator(&mut stream, &keypair).await?;

    run_sync_session(
        &mut stream,
        &mut noise,
        crdt,
        cache_dir,
        our_http_port,
        peer_http_base,
    )
    .await
}

// ---------------------------------------------------------------------------
// Inbound sync (responder side)
// ---------------------------------------------------------------------------

async fn handle_inbound_sync(
    mut stream: TcpStream,
    _peer_addr: SocketAddr,
    keypair: Arc<Keypair>,
    crdt: Arc<Mutex<NarInfoCrdt>>,
    cache_dir: PathBuf,
    our_http_port: u16,
) -> Result<(), SwarmError> {
    let mut noise = noise_handshake_responder(&mut stream, &keypair).await?;
    run_sync_session(
        &mut stream,
        &mut noise,
        crdt,
        cache_dir,
        our_http_port,
        // We don't know the peer's HTTP base URL until it tells us via
        // NarInfoData; pass an empty string and let the sync session fill it.
        "",
    )
    .await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Shared sync session logic (used by both sides)
// ---------------------------------------------------------------------------

/// Run a full CRDT sync exchange on an already-handshaked Noise session.
///
/// Returns the number of new narinfo entries accepted from the peer.
async fn run_sync_session(
    stream: &mut TcpStream,
    noise: &mut snow::TransportState,
    crdt: Arc<Mutex<NarInfoCrdt>>,
    cache_dir: PathBuf,
    our_http_port: u16,
    peer_http_base: &str,
) -> Result<usize, SwarmError> {
    let our_local_ip = stream.local_addr()?.ip();
    let our_http_base = format!("http://{our_local_ip}:{our_http_port}");

    // ---- Send our HaveList ----
    let have_entries: Vec<HaveEntry> = {
        let guard = crdt.lock().await;
        guard
            .have_list()
            .into_iter()
            .map(|(hash, timestamp)| HaveEntry { hash, timestamp })
            .collect()
    };

    send_encrypted(
        stream,
        noise,
        &SyncMsg::HaveList {
            entries: have_entries,
        },
    )
    .await?;

    // ---- Receive peer's HaveList, compute WantList ----
    let peer_have: SyncMsg = recv_encrypted(stream, noise).await?;
    let want_hashes = if let SyncMsg::HaveList { entries } = peer_have {
        let pairs: Vec<(String, u64)> =
            entries.into_iter().map(|e| (e.hash, e.timestamp)).collect();
        crdt.lock().await.want_from(&pairs)
    } else {
        vec![]
    };

    if want_hashes.is_empty() {
        send_encrypted(stream, noise, &SyncMsg::UpToDate).await?;
    } else {
        send_encrypted(
            stream,
            noise,
            &SyncMsg::WantList {
                hashes: want_hashes,
            },
        )
        .await?;
    }

    // ---- Receive peer's WantList, send requested entries ----
    let peer_want: SyncMsg = recv_encrypted(stream, noise).await?;
    if let SyncMsg::WantList { hashes } = peer_want {
        let guard = crdt.lock().await;
        for hash in hashes {
            if let Some(entry) = guard.get(&hash) {
                let msg = SyncMsg::NarInfoData {
                    hash: hash.clone(),
                    content: entry.content.clone(),
                    timestamp: entry.timestamp,
                    source_url: if entry.source_url.is_empty() {
                        our_http_base.clone()
                    } else {
                        entry.source_url.clone()
                    },
                };
                send_encrypted(stream, noise, &msg).await?;
            }
        }
    }
    send_encrypted(stream, noise, &SyncMsg::UpToDate).await?;

    // ---- Receive data for entries we wanted ----
    let mut new_paths = 0usize;
    loop {
        let msg: SyncMsg = recv_encrypted(stream, noise).await?;
        match msg {
            SyncMsg::UpToDate => break,
            SyncMsg::NarInfoData {
                hash,
                content,
                timestamp,
                source_url,
            } => {
                let effective_source = if source_url.is_empty() {
                    peer_http_base.to_string()
                } else {
                    source_url
                };

                let entry = CrdtEntry {
                    content: content.clone(),
                    timestamp,
                    source_url: effective_source,
                };

                let accepted = crdt.lock().await.insert(hash.clone(), entry);
                if accepted {
                    // Persist to disk.
                    let narinfo_path = cache_dir.join(format!("{hash}.narinfo"));
                    if let Err(e) = tokio::fs::write(&narinfo_path, &content).await {
                        warn!(%hash, "Failed to write narinfo: {e}");
                    } else {
                        debug!(%hash, "Wrote narinfo from peer");
                        new_paths += 1;
                    }
                }
            }
            _ => {
                debug!("Unexpected sync message; ending session");
                break;
            }
        }
    }

    Ok(new_paths)
}

// ---------------------------------------------------------------------------
// Now timestamp helper
// ---------------------------------------------------------------------------

/// Current Unix timestamp in seconds.
pub fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a minimal `SwarmNode` backed by a temp dir.
    async fn make_node(cache_dir: &std::path::Path, topic: &str, sync_port: u16) -> SwarmNode {
        let config = SwarmConfig {
            topic: topic.into(),
            discovery_port: 0, // not used in these unit tests
            sync_port,
            http_port: 5555,
            static_peers: vec![],
            ephemeral_keys: true,
        };
        SwarmNode::new(cache_dir.to_path_buf(), config)
            .await
            .unwrap()
    }

    #[test]
    fn test_now_secs_is_positive() {
        assert!(now_secs() > 0);
    }

    #[test]
    fn test_swarm_config_default() {
        let cfg = SwarmConfig::default();
        assert_eq!(cfg.discovery_port, 7070);
        assert_eq!(cfg.sync_port, 7071);
    }

    /// End-to-end test: two nodes sync a single narinfo over an in-process
    /// TCP connection without touching UDP multicast.
    #[tokio::test]
    async fn test_two_node_sync() {
        let dir_a = tempfile::tempdir().unwrap();
        let dir_b = tempfile::tempdir().unwrap();

        // Seed node A with one narinfo.
        let narinfo_content = "StorePath: /nix/store/abc123-hello-2.12\n\
                                   URL: nar/abc123.nar.xz\n\
                                   Compression: xz\n\
                                   FileHash: sha256:deadbeef\n\
                                   FileSize: 1024\n\
                                   NarHash: sha256:cafebabe\n\
                                   NarSize: 4096\n";
        tokio::fs::write(
            dir_a.path().join("abc123.narinfo"),
            narinfo_content.as_bytes(),
        )
        .await
        .unwrap();

        // Pick an ephemeral port for the sync server.
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let sync_port = listener.local_addr().unwrap().port();

        // Build the nodes.
        let node_a = make_node(dir_a.path(), "test-sync", sync_port).await;
        let node_b = make_node(dir_b.path(), "test-sync", sync_port + 1).await;

        let kp_a = Arc::clone(&node_a.keypair);
        let kp_b = Arc::clone(&node_b.keypair);
        let crdt_a = Arc::clone(&node_a.crdt);
        let crdt_b = Arc::clone(&node_b.crdt);
        let cache_a = node_a.cache_dir.clone();
        let cache_b = node_b.cache_dir.clone();

        // Spawn node A as the TCP responder on the pre-bound listener.
        tokio::spawn(async move {
            let (stream, peer_addr) = listener.accept().await.unwrap();
            handle_inbound_sync(stream, peer_addr, kp_a, crdt_a, cache_a, 5555)
                .await
                .unwrap();
        });

        // Wait a tick so the spawn is ready.
        tokio::time::sleep(Duration::from_millis(10)).await;

        // Node B initiates sync.
        let peer_addr: SocketAddr = format!("127.0.0.1:{sync_port}").parse().unwrap();
        let new_paths = initiate_sync(
            peer_addr,
            kp_b,
            Arc::clone(&crdt_b),
            cache_b.clone(),
            5556,
            "http://127.0.0.1:5555",
        )
        .await
        .unwrap();

        // Node B should have received the narinfo from node A.
        assert_eq!(new_paths, 1, "expected 1 new path synced to node B");
        assert!(
            cache_b.join("abc123.narinfo").exists(),
            "narinfo written to disk"
        );

        let guard = crdt_b.lock().await;
        assert!(guard.get("abc123").is_some());
    }
}
