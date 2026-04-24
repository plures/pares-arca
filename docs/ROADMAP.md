# Pares Arca (pares-cache) Roadmap

## Role in OASIS
Pares Arca is the free-tier entry point to the OASIS mesh. It provides a privacy-preserving, distributed cache for artifacts and content (build outputs, models, media, packages) so OASIS nodes can operate locally-first with fast, secure sharing.

## Current State
Architecture and product framing exist, but there is no production implementation in this repo yet. Core cache storage, P2P replication, and Nix integration are planned but not built.

## Phases

### Phase 1 — Core Cache & Sharing
- Implement PluresDB-backed artifact storage with content addressing.
- Add topic-key based peer discovery + replication over Hyperswarm.
- Integrate Nix substituter protocol for local/peer cache hits.
- Ship minimal CLI: create/join/list topics, cache stats.

### Phase 2 — Multi-Device Reliability
- Eviction policies (LRU/size/age) with tunable limits.
- Metrics and telemetry (hit rate, peer latency, storage usage).
- Bandwidth throttling and sync backpressure.
- macOS/Linux support with install hooks and system services.

### Phase 3 — OASIS Integration
- Signed artifact provenance + integrity verification.
- Praxis-driven cache policies (privacy, residency, access constraints).
- Cache-aware deployment pipeline for Rector + pares-nix.
- Content classes for OASIS (models, proofs, commerce bundles).

### Phase 4 — Marketplace & Scale
- Paid cache sharing tiers and quota enforcement.
- Cross-mesh replication and federation.
- Large-scale performance benchmarks and operational playbooks.
- Operator tooling for enterprise fleets.

## Status
🚧 Pre-alpha — core cache implementation required.
