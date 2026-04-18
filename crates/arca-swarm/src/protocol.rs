//! Wire protocol messages for Pares Arca P2P sync.
//!
//! The protocol has two layers:
//!
//! 1. **Discovery** — unencrypted UDP datagrams used to find peers sharing
//!    the same topic.  Peers announce themselves on a well-known multicast
//!    group and respond only to announcements whose `topic_hash` matches
//!    their own.
//!
//! 2. **Sync** — JSON messages sent inside Noise-encrypted TCP frames used
//!    to exchange narinfo metadata via the CRDT algorithm.

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Discovery (UDP, unencrypted)
// ---------------------------------------------------------------------------

/// Messages exchanged over UDP multicast for peer discovery.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum DiscoveryMsg {
    /// Broadcast: "I am here, sharing this topic."
    ///
    /// Only peers that know the same topic will recognise the hash and
    /// respond.  The hash is derived via `topic::derive_topic_hash` so the
    /// raw topic string is never sent on the wire.
    Announce {
        /// Hex-encoded SHA-256 of the topic (32 bytes → 64 chars).
        topic_hash: String,
        /// Hex-encoded Noise static public key (X25519, 32 bytes → 64 chars).
        noise_pubkey: String,
        /// TCP port on which we accept Noise-encrypted sync connections.
        sync_port: u16,
        /// HTTP port of our `arca-server` (for peers to fetch NAR files).
        http_port: u16,
    },

    /// Unicast reply to an `Announce` from a peer that shares the topic.
    AnnounceReply {
        /// Hex-encoded Noise static public key.
        noise_pubkey: String,
        /// TCP sync port.
        sync_port: u16,
        /// HTTP server port.
        http_port: u16,
    },

    /// UDP hole-punch probe.
    ///
    /// Sent to a peer's reported UDP address to open a NAT pinhole before
    /// the TCP sync connection is attempted.  The `our_addr` field lets the
    /// remote side learn our externally-visible address (useful when behind
    /// symmetric NAT with address-dependent mapping).
    HolePunch {
        /// Our externally-observed UDP address (IP:port string), if known.
        our_addr: Option<String>,
    },

    /// Acknowledge a `HolePunch` probe.
    HolePunchAck,
}

// ---------------------------------------------------------------------------
// Sync (TCP, Noise-encrypted)
// ---------------------------------------------------------------------------

/// Messages exchanged inside a Noise-encrypted TCP channel for CRDT sync.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SyncMsg {
    /// Advertise all narinfo entries this node knows about.
    ///
    /// The receiver compares the list against its own CRDT state and replies
    /// with a `WantList` for any entries it is missing or has an older
    /// version of.
    HaveList { entries: Vec<HaveEntry> },

    /// Request specific narinfo entries from the sender.
    WantList { hashes: Vec<String> },

    /// Deliver narinfo data for a specific store-path hash.
    NarInfoData {
        /// Hash portion of the store path (e.g. `"abc123xyz"`).
        hash: String,
        /// Full `.narinfo` text content.
        content: String,
        /// Unix timestamp (seconds) when the entry was last written.
        /// Used as the CRDT version for LWW resolution.
        timestamp: u64,
        /// Base HTTP URL of the originating node, e.g. `"http://10.0.0.5:5555"`.
        /// Nix will use this (rewritten into the narinfo `URL` field) to
        /// fetch the actual NAR archive.
        source_url: String,
    },

    /// Sent when the node has nothing new to offer after inspecting the
    /// peer's `HaveList`.
    UpToDate,
}

/// A single entry in a `HaveList` message.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HaveEntry {
    /// Store-path hash (the part before the first `-` in the basename).
    pub hash: String,
    /// Unix timestamp of the most recent write.
    pub timestamp: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_discovery_roundtrip() {
        let msg = DiscoveryMsg::Announce {
            topic_hash: "abc".into(),
            noise_pubkey: "def".into(),
            sync_port: 7071,
            http_port: 5555,
        };
        let json = serde_json::to_string(&msg).unwrap();
        let decoded: DiscoveryMsg = serde_json::from_str(&json).unwrap();
        if let DiscoveryMsg::Announce { sync_port, .. } = decoded {
            assert_eq!(sync_port, 7071);
        } else {
            panic!("wrong variant");
        }
    }

    #[test]
    fn test_sync_have_list_roundtrip() {
        let msg = SyncMsg::HaveList {
            entries: vec![
                HaveEntry {
                    hash: "abc123".into(),
                    timestamp: 1_700_000_000,
                },
                HaveEntry {
                    hash: "def456".into(),
                    timestamp: 1_700_000_001,
                },
            ],
        };
        let json = serde_json::to_string(&msg).unwrap();
        let decoded: SyncMsg = serde_json::from_str(&json).unwrap();
        if let SyncMsg::HaveList { entries } = decoded {
            assert_eq!(entries.len(), 2);
        } else {
            panic!("wrong variant");
        }
    }

    #[test]
    fn test_nar_info_data_roundtrip() {
        let msg = SyncMsg::NarInfoData {
            hash: "abc123".into(),
            content: "StorePath: /nix/store/abc123-hello\n".into(),
            timestamp: 9999,
            source_url: "http://10.0.0.2:5555".into(),
        };
        let json = serde_json::to_string(&msg).unwrap();
        let decoded: SyncMsg = serde_json::from_str(&json).unwrap();
        if let SyncMsg::NarInfoData { source_url, .. } = decoded {
            assert_eq!(source_url, "http://10.0.0.2:5555");
        } else {
            panic!("wrong variant");
        }
    }
}
