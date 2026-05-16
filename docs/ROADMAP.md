# Pares Cache (Arca) Roadmap

## Role in OASIS
Pares Cache is the free-tier entry point for OASIS. It provides a decentralized content cache for builds and artifacts using PluresDB + Hyperswarm, growing the mesh and lowering the barrier to join OASIS commerce.

## Current State
- Pre-alpha with architecture and design documented.
- Nix-oriented cache flow defined (topics, substituter, P2P replication).
- Implementation is minimal; core cache services not production-ready.

## Phase 1 — Core Cache MVP
- Implement PluresDB-backed artifact store and dedup pipeline.
- Build Hyperswarm replication with topic-based sharing.
- Provide Nix substituter interface + basic CLI.

## Phase 2 — Developer Experience
- NixOS module + post-build hook installer.
- Cache analytics (hit/miss, space savings).
- macOS support and basic performance tuning.

## Phase 3 — Mesh Expansion
- Capability discovery for other Pares services.
- Marketplace hooks for paid cache/compute exchanges.
- Security and privacy hardening (topic key governance).

## Phase 4 — Production Readiness
- Scale testing under multi-team workloads.
- Operational runbooks and reliability targets.
- Cross-platform packaging and docs.
