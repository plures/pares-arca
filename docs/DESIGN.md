# Pares Arca - Design Document

**Component**: Pares Arca ("strongbox") - Distributed P2P Nix Binary Cache
**Status**: Pre-alpha, Design Phase
**Last Updated**: 2026-02-20

> "Cache is the free draw. The mesh is the product. Every Pares node is PluresDB + Hyperswarm + capabilities."

## Overview

Pares Arca is the free-forever entry point to the Pares ecosystem. It provides a distributed, peer-to-peer Nix binary cache that completely eliminates the need for centralized cache services like FlakeHub. Every cache installation becomes a mesh node, creating a self-sustaining network effect.

### Core Principles

1. **Free Forever**: The cache will never cost money - it's the wedge that gets Pares ecosystem installed
2. **Local-First**: Works completely offline with your cached builds
3. **Privacy-First**: Nobody sees your builds except the peers you explicitly share with
4. **Mesh Participant**: Every installation contributes to the global Pares mesh

## Architecture

### Stack Overview

```
┌─────────────────────────────────────────┐
│           Nix Substituter               │
├─────────────────────────────────────────┤
│         Pares Arca Core                 │
├─────────────────────────────────────────┤
│    PluresDB (CRDT, Graph, Vectors)     │
├─────────────────────────────────────────┤
│      Hyperswarm (P2P Transport)        │
├─────────────────────────────────────────┤
│       Operating System (NixOS)         │
└─────────────────────────────────────────┘
```

### Data Flow

1. **Build Completion**: When Nix builds something locally
   - NAR + metadata stored in local PluresDB
   - Content-addressed by store path hash
   - Automatically tagged with build metadata

2. **P2P Replication**: Via Hyperswarm
   - Syncs to other machines with same topic key
   - Desktop ↔ laptop ↔ CI servers
   - Encrypted transport via Noise protocol

3. **Cache Retrieval**: When Nix needs a derivation
   - Check local PluresDB first
   - Query mesh peers if not found locally  
   - Fall back to upstream substituters
   - Update local cache with result

### Core Components

#### PluresDB Integration

Pares Arca uses PluresDB as its primary storage backend:

- **Content-Addressed Storage**: Store paths map to NAR files
- **Metadata Graph**: Dependencies, build provenance, popularity
- **Vector Search**: Semantic search across derivations and metadata
- **CRDT Sync**: Automatic replication across topic peers

```rust
pub struct CacheEntry {
    /// Nix store path (e.g., /nix/store/abc123-hello-2.12)
    pub store_path: String,
    /// Content hash of the NAR file
    pub nar_hash: [u8; 32],
    /// Compressed NAR data
    pub nar_data: Vec<u8>,
    /// Build metadata (derivation, dependencies, etc.)
    pub metadata: BuildMetadata,
    /// When this was cached
    pub cached_at: SystemTime,
    /// How many times served to peers
    pub serve_count: u64,
}
```

#### Hyperswarm Transport

All peer communication uses Hyperswarm:

- **Topic-Based Discovery**: Share a topic key = share a cache
- **Noise Encryption**: All connections encrypted end-to-end
- **NAT Traversal**: UDP hole-punching for direct peer connections
- **Fallback Support**: WebSocket relays for corporate firewalls

#### Nix Integration

Pares Arca implements the Nix substituter protocol:

```bash
# Automatic configuration during installation
nix.settings.substituters = [
  "https://cache.nixos.org"
  "pares://"
];
```

The `pares://` substituter queries the local PluresDB and mesh peers before falling back to upstream caches.

## Key Features

### 1. Zero Configuration Sharing

Share your cache with team members by sharing a topic key:

```bash
# Generate a team cache topic
pares cache create-topic --name="my-team"

# Share the topic key with team members
pares cache join-topic "a1b2c3d4e5f6..."
```

Everyone with the same topic key automatically shares their cached builds. No servers, no accounts, no configuration.

### 2. Multi-Device Sync

Your personal builds sync automatically across all your machines:

```
Desktop builds Firefox → Laptop gets it instantly
Laptop builds a project → CI server gets it instantly
CI builds deployment   → Both desktop + laptop get it
```

### 3. Intelligent Caching

The system learns which builds are popular and caches accordingly:

- **Hot content**: Popular derivations get cached on more nodes
- **Cold content**: Unpopular builds naturally expire
- **Pinned content**: Important builds can be pinned permanently

### 4. Privacy by Design

- No central authority sees your builds
- Peer discovery via cryptographic topic keys
- Optional I2P integration for maximum anonymity
- Content is encrypted in transit via Noise protocol

## Installation & Usage

### NixOS Integration

```nix
# /etc/nixos/configuration.nix
services.pares-arca = {
  enable = true;
  topics = [
    "personal"    # Your personal cross-device cache
    "team-alpha"  # Shared team cache
  ];
};
```

### Standalone Installation

```bash
# Install via Nix profile
nix profile install github:plures/pares-arca

# Or via the universal installer
curl -sSL install-pares.plures.io | sh
```

### Basic Commands

```bash
# Check cache status
pares cache status

# View cached derivations
pares cache list

# Clean old cache entries
pares cache clean --older-than=30d

# Export cache statistics
pares cache stats --format=json
```

## vs. Existing Solutions

### vs FlakeHub Cache

| Feature | FlakeHub | Pares Arca |
|---------|----------|------------|
| **Cost** | Paid plans | **Free forever** |
| **Privacy** | Central service sees all | **Private by design** |
| **Offline** | No cache access | **Always works locally** |
| **Sharing** | Platform-managed ACL | **Key-based peer sharing** |
| **Push model** | CI only | **Any peer** |
| **Infrastructure** | Centralized SaaS | **P2P mesh** |

### vs cache.nixos.org

| Feature | cache.nixos.org | Pares Arca |
|---------|-----------------|------------|
| **Scope** | nixpkgs only | **Any derivation** |
| **Speed** | Internet latency | **Local-first** |
| **Availability** | Single point of failure | **Distributed mesh** |
| **Custom builds** | Not cached | **Automatically cached** |

## Technical Implementation

### Cache Storage Format

NAR files are stored in PluresDB with the following schema:

```sql
-- Simplified representation (PluresDB uses graph structures)
CREATE TABLE cache_entries (
    store_path TEXT PRIMARY KEY,
    nar_hash BLOB NOT NULL,
    nar_size INTEGER NOT NULL,
    nar_data BLOB NOT NULL,
    build_time DATETIME,
    derivation_hash TEXT,
    metadata JSON
);

CREATE TABLE peer_availability (
    store_path TEXT,
    peer_id BLOB,
    last_seen DATETIME,
    latency_ms INTEGER
);
```

### Performance Characteristics

- **Local cache hits**: < 1ms (PluresDB query)
- **Peer cache hits**: 10-100ms (network round-trip)  
- **Cache miss**: Fall back to upstream (standard Nix behavior)
- **Storage efficiency**: CRDT deduplication + content compression

### Security Model

1. **Transport Security**: All peer connections use Noise protocol encryption
2. **Content Verification**: NAR hashes verified against Nix store expectations
3. **Peer Authentication**: Optional peer allowlists via topic key management
4. **Local Security**: Cache data encrypted at rest (configurable)

## Future Roadmap

### Phase 1: Core Cache (Current)
- [x] Basic PluresDB storage
- [x] Hyperswarm peer discovery
- [ ] Nix substituter implementation
- [ ] Topic-based sharing

### Phase 2: Enhanced Features
- [ ] Build provenance tracking
- [ ] Cache statistics and analytics  
- [ ] Performance optimization
- [ ] I2P privacy mode

### Phase 3: Mesh Integration
- [ ] Marketplace preparation (compute sharing)
- [ ] Advanced peer capabilities
- [ ] Cross-platform support (macOS, non-NixOS Linux)

## Configuration Reference

### Environment Variables

- `PARES_ARCA_DIR`: Cache storage directory (default: `~/.cache/pares`)
- `PARES_LOG_LEVEL`: Logging verbosity (default: `info`)
- `PARES_HYPERSWARM_KEY`: Override default topic key
- `PARES_OFFLINE_MODE`: Disable peer networking (default: `false`)

### Config File Format

```toml
# ~/.config/pares/cache.toml
[cache]
max_size_gb = 50
cleanup_interval = "24h"
compression_level = 6

[networking]
topics = ["personal", "team-alpha"]
max_peers = 50
connection_timeout = "30s"

[privacy]
i2p_mode = false
peer_allowlist = []
```

## Troubleshooting

### Common Issues

**Cache not sharing between machines:**
- Verify same topic key on both machines
- Check network connectivity (try `pares cache ping-peers`)
- Ensure Hyperswarm ports aren't blocked

**Slow cache performance:**
- Check available disk space
- Review cache size limits
- Monitor peer connection quality

**Nix substituter not working:**
- Verify substituter configuration in `nix.conf`
- Check Pares Arca service status
- Review logs with `pares cache logs`

---

*This design document reflects the current architecture plan. Implementation details may evolve during development.*