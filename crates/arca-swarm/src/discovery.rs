//! UDP multicast peer discovery and NAT hole-punching.
//!
//! Nodes broadcast `Announce` datagrams on a well-known IPv4 multicast
//! group.  Only peers whose `topic_hash` matches respond with
//! `AnnounceReply`, so the discovery channel is logically scoped to the
//! shared topic without revealing the topic string on the wire.
//!
//! ## Multicast group
//! `239.255.255.250` — the same address used by SSDP and commonly
//! allowed through home routers.  Port `7070` is used by default.
//!
//! ## NAT hole-punching
//! After learning a peer's address from a discovery reply the node sends
//! a `HolePunch` UDP datagram to open a NAT pinhole.  The peer replies
//! with `HolePunchAck`.  These probes are best-effort and do not block
//! the TCP sync connection attempt.

use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::time::Duration;

use tokio::net::UdpSocket;
use tracing::{debug, warn};

use crate::error::SwarmError;
use crate::protocol::DiscoveryMsg;

/// Multicast group address used for peer discovery.
pub const MULTICAST_ADDR: Ipv4Addr = Ipv4Addr::new(239, 255, 255, 250);

/// Default UDP port for discovery.
pub const DISCOVERY_PORT: u16 = 7070;

/// Timeout when waiting for at least one peer to reply.
pub const DISCOVERY_TIMEOUT: Duration = Duration::from_secs(5);

/// Interval between periodic re-announcements.
pub const ANNOUNCE_INTERVAL: Duration = Duration::from_secs(30);

// ---------------------------------------------------------------------------
// DiscoverySocket
// ---------------------------------------------------------------------------

/// Async UDP socket for peer discovery.
pub struct DiscoverySocket {
    socket: UdpSocket,
    multicast_addr: SocketAddr,
}

impl DiscoverySocket {
    /// Bind to `0.0.0.0:<port>` and join the multicast group.
    pub async fn bind(port: u16) -> Result<Self, SwarmError> {
        // Use SO_REUSEADDR so multiple processes can share the port during
        // testing (tokio's `UdpSocket::bind` enables this on most platforms).
        let socket = UdpSocket::bind(format!("0.0.0.0:{port}")).await?;

        socket.join_multicast_v4(MULTICAST_ADDR, Ipv4Addr::UNSPECIFIED)?;
        // Allow the socket to receive its own multicast packets (useful in
        // loopback tests where initiator and responder share an address).
        socket.set_multicast_loop_v4(true)?;

        let multicast_addr = SocketAddr::V4(SocketAddrV4::new(MULTICAST_ADDR, port));

        Ok(Self {
            socket,
            multicast_addr,
        })
    }

    /// Send a `DiscoveryMsg` to the multicast group.
    pub async fn multicast(&self, msg: &DiscoveryMsg) -> Result<(), SwarmError> {
        let data = serde_json::to_vec(msg)?;
        self.socket.send_to(&data, self.multicast_addr).await?;
        Ok(())
    }

    /// Send a `DiscoveryMsg` to a specific peer address (unicast).
    pub async fn send_to(&self, msg: &DiscoveryMsg, addr: SocketAddr) -> Result<(), SwarmError> {
        let data = serde_json::to_vec(msg)?;
        self.socket.send_to(&data, addr).await?;
        Ok(())
    }

    /// Receive the next `DiscoveryMsg` from any sender.
    ///
    /// Silently discards datagrams that cannot be deserialised (e.g. from
    /// unrelated SSDP traffic on the same multicast group).
    pub async fn recv(&self) -> Result<(DiscoveryMsg, SocketAddr), SwarmError> {
        let mut buf = [0u8; 2048];
        loop {
            let (n, addr) = self.socket.recv_from(&mut buf).await?;
            match serde_json::from_slice::<DiscoveryMsg>(&buf[..n]) {
                Ok(msg) => return Ok((msg, addr)),
                Err(e) => {
                    debug!("Discarding unrecognised datagram from {addr}: {e}");
                }
            }
        }
    }

    /// Send hole-punch probes to open a NAT pinhole towards `peer_addr`.
    ///
    /// Sends a `HolePunch` datagram and waits up to 500 ms for a
    /// `HolePunchAck` reply.  Failures are logged but do not propagate —
    /// hole-punching is best-effort.
    pub async fn hole_punch(
        &self,
        peer_addr: SocketAddr,
        our_addr: Option<String>,
    ) -> Result<(), SwarmError> {
        let probe = DiscoveryMsg::HolePunch { our_addr };
        self.send_to(&probe, peer_addr).await?;
        debug!("Sent hole-punch probe to {peer_addr}");

        // Best-effort: wait a short time for the ack.
        let timeout = tokio::time::timeout(Duration::from_millis(500), self.recv());
        match timeout.await {
            Ok(Ok((DiscoveryMsg::HolePunchAck, from))) => {
                debug!("Hole-punch ack from {from}");
            }
            Ok(Ok((other, from))) => {
                debug!("Expected HolePunchAck from {from}, got {other:?}");
            }
            Ok(Err(e)) => warn!("Hole-punch recv error: {e}"),
            Err(_) => debug!("Hole-punch ack timeout for {peer_addr}"),
        }
        Ok(())
    }

    /// Return our local socket address.
    pub fn local_addr(&self) -> Result<SocketAddr, SwarmError> {
        Ok(self.socket.local_addr()?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::topic::topic_hash_hex;

    /// Smoke-test: bind two sockets, send an Announce, receive it.
    #[tokio::test]
    async fn test_multicast_announce_recv() {
        // Pick an ephemeral port for the test to avoid collisions.
        let sender = DiscoverySocket::bind(0).await.unwrap();
        let receiver = DiscoverySocket::bind(0).await.unwrap();

        let recv_port = receiver.local_addr().unwrap().port();

        let announce = DiscoveryMsg::Announce {
            topic_hash: topic_hash_hex("test-topic"),
            noise_pubkey: "aabbcc".into(),
            sync_port: 7071,
            http_port: 5555,
        };

        // Send from sender's unicast socket directly to receiver (multicast
        // loop-back is platform-dependent in CI, so target unicast here).
        let recv_addr: SocketAddr = format!("127.0.0.1:{recv_port}").parse().unwrap();
        sender.send_to(&announce, recv_addr).await.unwrap();

        let (msg, _from) = tokio::time::timeout(Duration::from_secs(2), receiver.recv())
            .await
            .expect("recv timeout")
            .unwrap();

        if let DiscoveryMsg::Announce { sync_port, .. } = msg {
            assert_eq!(sync_port, 7071);
        } else {
            panic!("unexpected message type");
        }
    }
}
