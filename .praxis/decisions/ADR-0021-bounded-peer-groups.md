# ADR-0021: Bounded Peer Groups with Request Forwarding

**Status:** Accepted
**Date:** 2026-05-23
**Context:** Pares Arca swarm architecture for global P2P Nix substitute network

## Decision

Pares Arca uses bounded, unique peer groups with distance-limited request forwarding instead of a global DHT or full-mesh sync.

## Architecture

```
Distance 0: ME (1 node)
Distance 1: My peers (≤20 nodes) — direct connections
Distance 2: My peers' peers (~380 unique nodes) — known via index sharing
Distance 3: Their peers (~6,800 unique nodes) — reachable via forwarding

Total network reach within 3 hops: ~7,200 unique nodes
```

## Core Principles

### 1. Bounded Peer Group (max 20)

Each node maintains connections to at most 20 peers. This bounds:
- Memory (20 peer states + their indexes)
- Bandwidth (index sync with 20 nodes)
- CPU (20 connections to maintain)

### 2. Unique Group Membership

An algorithm ensures that peers within a group (distance ≤ 3) are **maximally unique** — minimal overlap between peer neighborhoods.

**Why:** If my 20 peers each have the same 20 peers, the network sees only 20 nodes. If each has 20 DIFFERENT peers, we see 400. Uniqueness maximizes coverage.

**Mechanism:**
- Each node shares a **peer list hash** (bloom filter of peer public keys) with its peers
- When selecting new peers, prefer candidates whose peer list hash has LOW overlap with your existing peers' lists
- Periodically prune peers that create too much redundancy and replace with better-coverage candidates

### 3. Index Sharing (What I Know About)

Every node shares its local **narinfo index** (set of store path hashes it can serve) with its direct peers.

Peers also share their **aggregated distance-2 index** — a compact summary of what THEIR peers have.

So every node knows:
- **Distance 1:** Exact paths available from each direct peer
- **Distance 2:** Bloom filter of paths available in peers' peer groups
- **Distance 3:** Bloom filter from distance-2 peers (lower confidence, higher false positive)

### 4. Request Forwarding (What I Need But Don't Have)

```
LOOKUP: Need /nix/store/abc123-foo

1. Check local cache → found? Serve it. Done.
2. Check distance-1 index → peer X has it? Fetch from X. Done.
3. Check distance-2 bloom → likely in peer Y's group?
   → Send REQUEST to peer Y
   → Y checks its peers → found at peer Z
   → Z serves to Y → Y caches → Y serves to me. Done.
4. Check distance-3 bloom → might exist further?
   → Send REQUEST to peer with best bloom match
   → Forwarded through one more hop (same mechanism)
   → Package propagates back along the chain. Done.
5. Nothing found → fall back to cache.nixos.org. Done.
```

### 5. Package Migration (Demand-Driven Caching)

When a request is fulfilled, the package is cached at EVERY node along the return path. This means:
- Popular packages migrate toward clusters of demand
- After first fetch, the package is in my group — all future requests are local
- The network self-optimizes for access patterns

## Peer Selection Algorithm

```
ALGORITHM: SelectPeer(candidate)

INPUT:
  - candidate: a potential new peer
  - my_peers: current peer set (≤20)
  - peer_blooms: bloom filters of each peer's peer list

SCORE(candidate):
  overlap = 0
  for peer in my_peers:
    overlap += bloom_intersection_estimate(peer_blooms[peer], candidate.peer_bloom)
  
  # Lower overlap = better (more unique coverage)
  uniqueness = 1.0 - (overlap / (|my_peers| * candidate.peer_bloom.count()))
  
  # Prefer peers with large indexes (they have more packages)
  richness = candidate.index_size / MAX_INDEX_SIZE
  
  # Prefer peers with low latency
  proximity = 1.0 / candidate.rtt_ms
  
  RETURN 0.5 * uniqueness + 0.3 * richness + 0.2 * proximity

MAINTENANCE (periodic, every 5 min):
  for peer in my_peers:
    if peer.last_seen > TIMEOUT:
      remove(peer)
      find_replacement(prefer high SCORE)
  
  # Check for redundancy creep
  worst = min(my_peers, key=SCORE)
  candidate = random_from_dht()
  if SCORE(candidate) > SCORE(worst) * 1.5:
    replace(worst, candidate)
```

## Index Data Structures

```rust
/// What a node knows about packages in its vicinity.
struct PeerIndex {
    /// Exact store path hashes this peer can serve (distance 1).
    local_paths: HashSet<[u8; 20]>,  // truncated SHA-256
    
    /// Bloom filter summarizing distance-2 paths.
    /// ~100K paths in 128KB bloom → 0.1% false positive rate.
    distance_2_bloom: BloomFilter,
    
    /// Bloom filter summarizing distance-3 paths (lower confidence).
    distance_3_bloom: BloomFilter,
}

/// Shared between peers during index sync.
struct IndexAnnouncement {
    /// My exact local paths (compact: 20 bytes per path).
    paths: Vec<[u8; 20]>,
    
    /// Bloom filter of my peers' paths (for distance-2 visibility).
    peer_bloom: BloomFilter,
    
    /// Bloom filter of my peer list (for uniqueness algorithm).
    peer_list_bloom: BloomFilter,
    
    /// Number of paths in my local cache (for richness scoring).
    index_size: u32,
}
```

## Request Protocol

```rust
enum SwarmMessage {
    /// Index sync between direct peers (periodic, compact).
    IndexSync(IndexAnnouncement),
    
    /// "I need this store path" — forwarded up to 2 hops.
    Request {
        /// Truncated hash of the store path needed.
        path_hash: [u8; 20],
        /// Who originally requested (for return routing).
        origin: PeerId,
        /// Remaining hops allowed (starts at 2, decremented each forward).
        ttl: u8,
        /// Request ID for deduplication.
        request_id: [u8; 16],
    },
    
    /// "I have it, here's where to get it."
    Response {
        path_hash: [u8; 20],
        request_id: [u8; 16],
        /// HTTP endpoint to fetch the NAR from.
        source_url: String,
    },
    
    /// Direct NAR transfer (for peers that can't expose HTTP).
    NarTransfer {
        path_hash: [u8; 20],
        narinfo: Vec<u8>,
        nar_data: Vec<u8>,
    },
}
```

## Trust Model

### Public packages (from nixpkgs / any trusted cache):
- NARs carry upstream signatures (e.g. `cache.nixos.org-1:...`)
- Nix verifies automatically — pares-arca serves bytes, trusts nothing
- Any peer can serve these safely — cryptographic guarantee from Nix
- **Zero configuration needed**

### Private packages (user-built):
- User generates signing key: `pares-arca keygen`
- Shares public key with trusted peers
- Only announced within explicit private topic (not global network)
- Consumers must add key to `trusted-public-keys`

### Attack resistance:
- **Malicious NARs:** Nix rejects anything without a valid signature from `trusted-public-keys`. A peer serving tampered data is harmlessly ignored.
- **Eclipse attacks:** The uniqueness algorithm ensures diverse peer selection. An attacker would need to control >50% of nodes in a 3-hop radius.
- **Sybil attacks:** Rate-limit new peer acceptance. Require proof-of-cache (must actually serve NARs to stay in peer group).
- **Index poisoning:** Bloom filter claims are probabilistic — false positives waste one request but don't compromise security. A peer that repeatedly fails to serve what it claims gets deprioritized.

## Network Bootstrap

1. First start: connect to a small set of hardcoded bootstrap nodes (like BitTorrent trackers)
2. Bootstrap nodes are just regular peers with high uptime — they provide initial peer candidates
3. Once connected to 1 peer, receive its peer list → expand using the selection algorithm
4. Within seconds: 20 peers, visibility into thousands of packages
5. Optional: Mainline DHT as a secondary discovery mechanism (no single point of failure)

## Performance Estimates

| Metric | Value |
|---|---|
| Peers per node | 20 (bounded) |
| Unique nodes visible (distance 3) | ~7,200 |
| Index sync bandwidth | ~2MB/min (20 peers × compressed narinfo hashes) |
| Request latency (distance 1) | <100ms (direct TCP) |
| Request latency (distance 2) | <500ms (1 hop forward + fetch) |
| Request latency (distance 3) | <2s (2 hop forward + fetch) |
| Fallback to cache.nixos.org | Only if not found in 7,200 nodes |

## Migration Path

### Phase 1: LAN-only (current)
- Keep multicast discovery for same-subnet peers
- These become "free" distance-1 peers (zero latency)

### Phase 2: Static peers + basic forwarding
- Add `--static-peer` for known remote peers (home lab ↔ office)
- Implement request forwarding between peer groups
- Test on praxisbot ↔ surface

### Phase 3: DHT bootstrap + global network
- Add Mainline DHT or dedicated bootstrap nodes
- Implement peer selection algorithm with bloom filters
- Release as public network — anyone can join

### Phase 4: Scale
- Optimize index sync (delta encoding, compressed blooms)
- Add reputation scoring (peers that serve get priority)
- Geographic proximity weighting

## What This Replaces

- ❌ `nix.settings.trusted-public-keys` for pares-arca keys (public packages don't need it)
- ❌ Shared topic configuration (global network = one topic)
- ❌ Static peer lists (automatic peer selection)
- ❌ LAN-only limitation (global reach)
- ❌ Push-all narinfo sync (pull-on-demand)

The NixOS module becomes:

```nix
services.pares-arca = {
  enable = true;        # That's it. Zero config for public packages.
  postBuildHook = true; # Contribute your builds to the network.
};
```
