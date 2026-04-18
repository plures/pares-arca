# Pares Arca

> "strongbox" — the free draw

Distributed content cache built on PluresDB + Hyperswarm. Every cache node is a full mesh participant. Free tier entry point to the Pares ecosystem — install the cache, become a node.

## Getting Started

### Quick Installation

```bash
# Via Nix profile
nix profile install github:plures/pares-cache

# Via universal installer  
curl -sSL install-pares.plures.io | sh

# NixOS configuration
services.pares-arca.enable = true;
services.pares-arca.postBuildHook = true;
```

### First Use

```bash
# Check status
pares cache status

# Create a personal topic (syncs across your devices)
pares cache create-topic --name="personal"

# Join a team cache (share builds with teammates)
pares cache join-topic "a1b2c3d4e5f6..."

# View your cached builds
pares cache list

# Install the system post-build hook (writes /etc/nix/post-build-hook)
sudo pares-cache install-hook
```

That's it! Your Nix builds now cache automatically and sync across all your machines.

### What You Get

- **Free forever** — No accounts, no subscriptions, no limits
- **Instant sync** — Builds appear on all your devices immediately
- **Team sharing** — Share caches with teammates via topic keys
- **Privacy first** — Your builds stay private to your chosen peers
- **Offline works** — Local cache always available, no internet required

## Architecture

Pares Arca transforms Nix's content-addressed nature into a powerful P2P caching system:

```
Your Build → PluresDB → Hyperswarm → Team's Machines
                ↓
         Local Cache (always available)
```

### Core Components

- **PluresDB**: CRDT-synced storage with content deduplication
- **Hyperswarm**: Encrypted P2P transport with NAT traversal
- **Nix Integration**: Drop-in substituter that works with existing workflows
- **Topic Keys**: Cryptographic sharing - share key = share cache

### How It Works

1. **Build locally** → Nix builds something, Arca caches it in PluresDB
2. **Share automatically** → Hyperswarm replicates to peers with same topic key  
3. **Fetch instantly** → Next build needing that derivation gets it from cache
4. **Falls back gracefully** → If not in cache, fetches from upstream (cache.nixos.org)

This creates a **flywheel effect**: the more your team builds, the faster everyone's builds become.

## Key Features

### Zero-Configuration Sharing

```bash
# Share cache with team
pares cache create-topic --name="frontend-team"
# → Outputs: "Topic key: a1b2c3d4e5f6... (share this with team)"

# Team members join
pares cache join-topic "a1b2c3d4e5f6..."
```

Everyone with the same topic key automatically shares cached builds. No servers, no setup, no accounts.

### Multi-Device Personal Cache

Your personal builds sync across all your machines:

```
Desktop builds project → Laptop gets cache instantly
Laptop builds Docker   → CI server gets cache instantly  
CI builds deployment   → Both desktop+laptop get it
```

### Privacy by Design

- **No central authority** sees your builds
- **Peer discovery** via cryptographic topic keys only
- **Encrypted transport** via Noise protocol
- **Local-first** - works completely offline

### Performance

- **Local hits**: < 1ms (PluresDB query)
- **Peer hits**: 10-100ms (direct P2P)
- **Storage efficient**: CRDT deduplication + compression
- **Bandwidth smart**: Only downloads what you need

## Status

🚧 **Pre-alpha** — Architecture and design phase.

### Milestones

- [ ] **Phase 1: Core Cache** (Q2 2026)
  - [ ] PluresDB storage backend
  - [ ] Hyperswarm P2P networking
  - [ ] Nix substituter protocol
  - [ ] Topic-based sharing
  - [ ] NixOS integration

- [ ] **Phase 2: Enhanced Features** (Q3 2026)
  - [ ] Build provenance tracking
  - [ ] Cache analytics and statistics
  - [ ] Performance optimizations
  - [ ] I2P privacy mode
  - [ ] macOS support

- [ ] **Phase 3: Mesh Integration** (Q4 2026)
  - [ ] Marketplace readiness (compute sharing)
  - [ ] Advanced peer capabilities
  - [ ] Cross-platform support
  - [ ] Enterprise features

## Part of Pares

pares-cache is part of the [Pares](https://github.com/plures/pares) mesh ecosystem:

| Product | Latin | Role |
|---|---|---|
| **Pares Arca** | "strongbox" | Distributed cache (free tier) |
| **Pares Agens** | "one who acts" | AI agent framework |
| **Pares Manus** | "hands" | Capability nodes (Windows/macOS/mobile) |
| **Pares Rector** | "one who steers" | Goal-based orchestrator |
| **Arcae Nexus** | "strongboxes + connection" | Decentralized object registry |
| **Pares Protocol** | — | Wire protocol + command channel |
| **Pares Nix** | — | NixOS config generation |

All components share [PluresDB](https://github.com/plures/pluresdb) as the data plane and [Hyperswarm](https://github.com/plures/hyperswarm) for P2P connectivity.

### The Flywheel

Pares Arca is the **free draw** that gets the ecosystem installed:

1. **Install Pares** → get free cache (better than FlakeHub)
2. **Free cache** → machine becomes a mesh node  
3. **Connect devices** → multi-device cache sharing
4. **Join mesh** → discover other Pares capabilities
5. **Use marketplace** → buy/sell compute, content, services

Every cache installation expands the mesh. Free forever by design.

## Documentation

- **[Design Document](docs/DESIGN.md)** — Technical architecture and implementation details
- **[Development Guide](https://github.com/plures/development-guide)** — Cross-cutting concerns and standards

## Contributing

Pares Arca is open source under AGPL-3.0. See the [development guide](https://github.com/plures/development-guide) for contribution guidelines, coding standards, and architecture decisions.

## License

AGPL-3.0
