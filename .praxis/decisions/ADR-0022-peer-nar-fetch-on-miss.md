# ADR-0022: Peer NAR Fetch on Local Cache Miss

**Status:** Proposed
**Date:** 2026-07-23
**Context:** Epic `pares-arca:cli-and-nix-substituter` (issues #10, #11). #11 (CLI: topics/cache-stats) shipped in PR #17. This ADR scopes the remaining work for #10 (Nix substituter protocol for local/peer cache hits).

## Problem

Today the substituter HTTP server (`arca-server`) and the swarm sync (`arca-core::sync`) are only loosely connected:

- **Narinfo metadata** replicates proactively to all peers on a topic via PluresDB/Hyperswarm CRDT sync (`start_sync`, wired into `serve --sync-topic`).
- **NAR blobs** are *not* replicated or forwarded. `GET /nar/<file>` (`arca-server/src/lib.rs`) only checks the local `NarObjectStore`, then falls back to the legacy on-disk `nar/` directory. If neither has the blob, the client gets a 404.

This creates an inconsistent state that violates the documented design ("Only narinfo metadata is proactively replicated — actual build outputs are fetched lazily", `docs/CACHE-CONFIGURATION.md`): a node can learn via swarm sync that a peer has built and cached a store path (narinfo present, `Sig:` valid), advertise it to local Nix as substitutable, and then fail to actually serve the NAR because it never fetched the underlying blob from the peer that has it. Nix will report a substitution failure and fall back to building locally, defeating the purpose of the cache.

This is the core remaining gap in the "Nix substituter protocol for local/peer cache hits" epic item (#10). The CLI/topics work (#11 / PR #17) is complete and does not depend on this.

## Decision (proposed)

Add an on-demand peer NAR fetch path to `arca-server`, scoped to the bounded peer group already established by ADR-0021:

1. **Peer registry**: `arca-core::sync` gains a lightweight `PeerRegistry` (in-memory, per-topic) recording which connected peers have advertised which store-path hashes via narinfo CRDT sync. This piggybacks on the existing `Replicator`/`GunMessage::Put` exchange — no new wire protocol, just indexing what's already received.
2. **Miss handling in `nar_file`**: On a local NAR miss, before returning 404, `arca-server` consults the `PeerRegistry` for peers advertising that hash. If found, it opens a short-lived NAR-fetch request over the existing swarm transport (reusing the Noise XX encrypted TCP connection already open to that peer — no new socket/port) requesting the blob by content hash.
3. **Fetch-then-cache**: On success, the fetched NAR is written into the local `NarObjectStore` (so subsequent requests are local hits and dedup/GC apply normally), then streamed to the original HTTP client. A single in-flight de-dup guard (per hash) prevents thundering-herd refetch when multiple local Nix clients request the same missing path concurrently.
4. **Bounded fan-out**: Fetch attempts are limited to the bounded peer group (max ~20 direct peers per ADR-0021) with a fixed timeout (e.g., 5s per peer, matching the existing sync receive timeout) and no forwarding beyond distance 1 in this phase — i.e., "ask my peers", not "ask my peers' peers". Multi-hop forwarding (distance 2–3, per ADR-0021's stated network reach) is out of scope for this ADR and tracked as a possible phase 2 follow-up.
5. **Failure semantics**: If no peer serves the blob within the timeout, return 404 as today (no behavior regression) but log a `narinfo_without_nar` warning metric so operators can detect drift between narinfo and blob availability.
6. **CLI/observability surface**: Extend `cache-stats --json` (from PR #17) with `peer_fetches`, `peer_fetch_misses` counters wired the same way as the existing `narinfo_hits`/`narinfo_misses` atomics in `AppState`, so `pares-arca cache-stats` reports peer-fetch activity without a separate command.

## Non-goals (explicitly deferred)

- Multi-hop (distance 2–3) forwarding — deferred; would need forwarding logic and loop prevention beyond this ADR's scope.
- A dedicated NAR-fetch wire message type distinct from the existing sync transport — reusing existing Noise XX connections keeps this additive rather than a new protocol surface.
- Changes to `topics create/join/list/remove` or `cache-stats` command shape beyond the two new counters above — that CLI surface is considered complete per PR #17.

## Consequences

- Closes the correctness gap between "narinfo says I have it" and "I can actually serve it," which is required for the substituter to be usable in the public/private swarm scenarios documented in `docs/CACHE-CONFIGURATION.md`.
- No new listening ports; reuses the sync transport already established for narinfo replication, keeping firewall/NixOS module surface (`openFirewall`, discovery/sync ports) unchanged.
- Adds moderate complexity to `arca-server` (needs access to the peer registry / sync transport, currently owned by the CLI's `serve` command wiring) — implementation will need `serve()` to accept an optional peer-fetch handle alongside `sync_topic`, similar to how `nar_store` is already threaded through.

## Epic status

With this ADR accepted and implemented, `pares-arca:cli-and-nix-substituter` (#10, #11) would be complete. #11 is already done (PR #17, unmerged as of this writing — no code changes needed, just needs review/merge). #10 requires the peer-NAR-fetch work above; everything else in the "Nix substituter protocol" (nix-cache-info, narinfo, signing, compression, GC) already shipped prior to this epic per CHANGELOG 1.1.x/1.2.x.

No implementation is included in this ADR — it is design-only, per epic sequencing (CLI first, substituter integration second).
