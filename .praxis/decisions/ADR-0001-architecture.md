# ADR-0001: Segmented Caches with PluresDB Metadata + plures-object Blob Storage

## Status

Accepted — 2026-04-24

## Context

pares-cache currently uses a plain filesystem layout for narinfo and NAR files. The README referenced PluresDB integration but this was aspirational — no PluresDB code exists. The architecture needs to support:

1. Multiple independent cache segments sharing one daemon
2. Peer-to-peer replication without a central server
3. Content-addressed, deduplicated blob storage
4. Audit trail and CRDT-based metadata convergence

## Decision

### Two cache segments

- **Universal** — nixpkgs and well-known public packages. Ships with a well-known topic key. This is the default segment and what most users interact with.
- **Custom** — user-generated packages with user-generated topic keys. Requires explicit `pares-cache keygen` to create a topic key, shared out-of-band.

### Storage split

- **PluresDB** stores all metadata: narinfo records, CRDT merge state, peer registry, and audit trail (via Chronos). PluresDB's Hyperswarm transport handles peer discovery and encrypted replication.
- **plures-object** stores NAR blobs. Content-addressed, chunked, and deduplicated. PluresDB metadata references blob keys in plures-object.

### Replication

PluresDB Hyperswarm replaces the custom `arca-swarm` discovery and sync protocol. Encryption is handled by PluresDB at the replication layer, not at the transport layer. The `arca-swarm` crate becomes a thin adapter that configures PluresDB Hyperswarm with cache-specific topics.

### Post-build hook routing

The Nix post-build hook determines which segment a store path belongs to:
- Paths from the nixpkgs closure route to the universal segment
- All other paths route to the custom segment (or default if no custom segment is configured)

## Consequences

- `arca-swarm` crate becomes a thin adapter over PluresDB Hyperswarm. Custom UDP discovery, Noise handshake, and TCP sync code is removed.
- Filesystem-only `CacheStore` is replaced with a PluresDB-backed metadata store + plures-object blob store.
- Significant refactor of `arca-core` — the `store.rs` module needs to understand segments and delegate to the correct backends.
- Config file required to define segments and their topic keys.

## Evidence

| Fact | Source | Tested? |
|------|--------|---------|
| PluresDB Hyperswarm handles topic-based peer discovery | pluresdb docs | Yes (other plures projects) |
| plures-object supports content-addressed chunked storage | plures-object v0.1 shipped 2026-03-18 | Yes |
| Chronos provides audit trail over PluresDB | chronos v0.1 shipped 2026-03-18 | Yes |
| Current arca-swarm uses custom UDP+TCP protocol | arca-swarm source | Yes |
| Current store is filesystem-only | arca-core/src/store.rs | Yes |

## Unknowns

- PluresDB Hyperswarm performance under high NAR throughput (needs benchmarking)
- plures-object chunk size tuning for typical NAR sizes
- Whether narinfo CRDT merge conflicts are possible in practice (theoretically no — narinfo is immutable per store path hash)
