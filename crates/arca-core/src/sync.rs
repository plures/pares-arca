//! PluresDB Hyperswarm sync for the NAR index.
//!
//! When enabled, the CrdtStore's narinfo metadata replicates to all peers
//! on the same topic via PluresDB's built-in Hyperswarm transport.

use std::sync::Arc;

use pluresdb::CrdtStore;
use pluresdb_sync::{create_transport, GunMessage, Replicator, TransportConfig, TransportMode};
use tokio::task::JoinHandle;
use tracing::{debug, error, info, warn};

/// Actor ID used for sync replication writes.
const SYNC_ACTOR: &str = "pares-arca-sync";

/// Start Hyperswarm sync for a CrdtStore with the given topic.
///
/// The topic is hashed to produce a 32-byte Hyperswarm topic key.
/// All peers using the same topic string will discover each other and
/// replicate CrdtStore contents automatically.
///
/// Returns a JoinHandle for the background sync task.
pub fn start_sync(
    store: Arc<CrdtStore>,
    topic: &str,
) -> Result<JoinHandle<()>, String> {
    let topic_key = derive_topic_key(topic);
    let runtime = tokio::runtime::Handle::try_current()
        .map_err(|e| format!("start_sync requires an active Tokio runtime: {e}"))?;

    info!(topic = %topic, "Starting PluresDB Hyperswarm sync");

    Ok(runtime.spawn(async move {
        let mut transport = create_transport(TransportConfig {
            mode: TransportMode::Hyperswarm,
            ..Default::default()
        });

        let mut connections = match transport.connect(topic_key).await {
            Ok(rx) => rx,
            Err(e) => {
                error!("Failed to connect Hyperswarm transport: {e}");
                return;
            }
        };

        info!("PluresDB Hyperswarm sync active — waiting for peers");

        while let Some(mut connection) = connections.recv().await {
            let store = Arc::clone(&store);
            tokio::spawn(async move {
                if let Err(e) = handle_sync_connection(store, &mut *connection).await {
                    warn!("Sync connection failed: {e}");
                }
            });
        }
    }))
}

async fn handle_sync_connection(
    store: Arc<CrdtStore>,
    connection: &mut dyn pluresdb_sync::Connection,
) -> Result<(), String> {
    let replicator = Replicator::new(SYNC_ACTOR);

    // Send all our local records to the peer
    for record in store.list() {
        let payload = replicator
            .encode_put(&record.id, record.data.clone())
            .map_err(|e| format!("encode_put failed: {e}"))?;
        connection
            .send(&payload)
            .await
            .map_err(|e| format!("send failed: {e}"))?;
    }

    // Signal we're done sending
    connection
        .close()
        .await
        .map_err(|e| format!("close failed: {e}"))?;

    // Receive records from the peer
    loop {
        let maybe_payload = match tokio::time::timeout(
            std::time::Duration::from_secs(5),
            connection.receive(),
        )
        .await
        {
            Ok(result) => result.map_err(|e| format!("receive failed: {e}"))?,
            Err(_) => {
                debug!("sync receive timeout reached; ending peer sync loop");
                break;
            }
        };

        let Some(payload) = maybe_payload else {
            break;
        };

        let message =
            GunMessage::decode(&payload).map_err(|e| format!("decode failed: {e}"))?;

        if let GunMessage::Put(put) = message {
            for (id, node) in put.put {
                let value = serde_json::Value::Object(node.fields.into_iter().collect());
                store.put(id, SYNC_ACTOR, value);
            }
        }
    }

    info!("Peer sync complete");
    Ok(())
}

/// Derive a 32-byte topic key from a human-readable topic string.
fn derive_topic_key(topic: &str) -> [u8; 32] {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(b"pares-arca-topic:");
    hasher.update(topic.as_bytes());
    hasher.finalize().into()
}
