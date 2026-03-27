# Pares Arca (pares-cache) Roadmap

## Role in Pares Ecosystem
Pares Arca is the distributed cache and free-tier onramp for the Pares mesh. It turns Nix builds into shareable, encrypted artifacts stored in PluresDB and replicated over Hyperswarm, making the mesh useful on day one.

## Current State
The repo provides architecture and product framing, but no source implementation is present (no src tree). Core capabilities like cache eviction, PluresDB storage, and Hyperswarm discovery are planned but not built.

## Milestones

### Near-term (Q2 2026)
- Implement PluresDB-backed artifact storage with content addressing.
- Integrate with Nix substituter protocol for local cache hits.
- Establish topic-key discovery and peer replication over Hyperswarm.
- Add basic cache statistics and CLI status commands.

### Mid-term (Q3-Q4 2026)
- Add cache eviction policies (LRU/size/age) with configurable limits.
- Build bandwidth management + throttling for sync.
- Implement metrics export (hit ratio, peer latency, storage usage).
- Support multi-device “personal cache” sync and team topics.

### Long-term
- Marketplace-grade reliability and provenance tracking.
- Cross-platform support and hardened security posture.
- Performance benchmarks at mesh scale and production documentation.
