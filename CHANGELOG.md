## [1.2.1] — 2026-07-23

- fix(nix): preserve upstream substituters when enabling Arca (#18) (1d8bf2b)

## [1.2.0] — 2026-07-23

- ci(release): trigger release pipeline on merge to main (#16) (1428aa9)
- ci: migrate Tech Doc Writer to shared reusable (fd92648)
- fix(ci): repair tech-doc-writer YAML indentation / remove empty workflow (3ea4f02)
- ci: add security-aware Dependabot auto-merge workflow (org backfill) (5836a8f)
- ci: lifecycle cron */30 -> 0 */2 (cut scheduled Actions spend; events still real-time) (25a2567)
- fix(ci): point copilot-pr-lifecycle at public plures/.github reusable workflow (33a27e1)
- feat(cli): add --force to sign command for re-signing existing narinfos (40fce44)
- fix(signing): use nix-base32 encoding in narinfo fingerprints (72546a3)
- feat(cli): support multiple --sync-topic flags (#15) (752aa43)
- feat(nix): layered sync with secure defaults (#14) (3ec0feb)
- fix(nix): use secret-key-files for zero-config trust (no require-sigs=false) (#13) (03300b0)
- fix(nix): zero-config local mode - disable require-sigs by default (#12) (5987375)
- feat: wire PluresDB Hyperswarm sync into serve command (Phase 3) (fe4f7d4)
- refactor: replace sled index + arca-swarm with PluresDB CrdtStore (d03db5a)
- feat(nix): add swarm support to NixOS module — zero manual config (6f69712)
- fix(swarm): add periodic re-sync every 60s (78db210)
- ADR-0021: bounded peer groups with request forwarding (c0cc9fa)
- fix(nix): use extra-trusted-public-keys instead of trusted-public-keys (cdc24b2)
- feat: add operational logging + debug level switch (97333eb)
- fix: ensure signing dir/public key/trust conf are world-readable (c19b371)
- fix(signing): prefix /nix/store/ on references in fingerprint (f67e0af)
- fix(module): run ExecStartPre sign as root with + prefix (128f887)
- fix(module): use LoadCredential for signing key access (897a703)
- feat(module): auto-sign unsigned narinfos on service start (3a94296)
- feat: add 'sign' command to bulk-sign unsigned narinfos (623ad61)
- chore(release): v0.3.1 — trigger rebuild to test cache signing (4e10f9a)
- fix(module): sign narinfos during import, not in local store (e2bf3ed)
- fix: eliminate double sled open — single NarObjectStore per process (0a2756c)
- fix: fully resilient startup — FileBlobStore tempdir fallback + tested recovery (89ae626)
- fix: unique tempdir per NarObjectStore instance (avoid sled lock conflict) (dc1161f)
- fix: self-heal FileBlobStore on permission/corruption errors (8f6f9c9)
- fix: never panic on sled index failure — progressive recovery with tempdir fallback (e13bfc7)
- fix: merge extraOptions into single block (duplicate attribute error) (f94e80a)
- fix: move post-build-hook to nix.extraOptions (not valid in nix.settings) (820a815)
- refactor: rename pares-cache to pares-arca (naming standardization) (7884ea7)
- fix: use localhost for substituter URL, always regenerate trust config (b5835b4)
- ci: add path filters to CI workflow (7d6bb96)
- ci: change release trigger from push-to-main to tag-only (1084678)
- docs: add cache configuration guide (local, public, private) (9c27401)
- feat: auto-generate signing key pair on first activation (1e3fdd6)
- fix: recover from corrupted/locked sled index instead of panicking (7750992)
- fix: use __noChroot instead of outputHashes for git deps (f65b230)
- fix: correct pluresdb outputHash for Nix sandbox build (c25564e)
- fix: use outputHashes for pluresdb git dep in flake.nix (78a4f2e)
- fix: resolve all clippy warnings (2ba2dd1)
- feat: migrate NarObjectStore from plures-object to pluresdb (2bc0d58)
- fix: pin plures-object to rev, remove unused pluresdb deps (b928547)
- fix: restore plures-object deps for NarObjectStore (3b7a8fb)
- style: cargo fmt (3af519f)
- fix: add bytes to workspace dependencies (d8b2bbc)
- feat(nix): add secretKeyFile option for NAR signing (451d08f)
- wip: save work in progress (3b52a3a)

## [1.1.5] — 2026-05-11

- refactor: replace inline lifecycle with reusable workflow call (679ad66)

## [1.1.4] — 2026-05-02

- fix: sled double-open lock panic — reuse global backend in serve (f3e0fa4)

## [1.1.3] — 2026-05-02

- fix: default backend to filesystem (sled lock panics on restart) (cb675e6)

## [1.1.2] — 2026-05-01

- fix: suppress ci-feedback issue spam (24h dedup window) (5500493)

## [1.1.1] — 2026-04-25

- fix: add allowBuiltinFetchGit for plures-object git dependency (a94aec7)

## [1.1.0] — 2026-04-25

- feat: zstd compression, garbage collection, v1.0.0 (de830b6)
- feat: ed25519 narinfo signing (Phase 0.6a) (67b8e15)
- feat: integrate plures-object for content-addressed NAR blob storage (phase 0.5) (ce60904)
- feat: wire swarm --also-serve to use CacheBackend, default backend to sled (bdd52e7)
- feat: wire HTTP server to use CacheBackend trait (7f9dd18)

## [0.4.0] — 2026-04-25

- docs: add phase 0.4 praxis expectations + README backend docs (5c5be02)
- feat: wire --backend and --db-path flags into CLI (1a1f06f)
- test: add backend conformance tests for filesystem and sled (6deee11)
- feat: add append-only audit log backed by sled (6a9e32a)
- feat: add sled-backed CacheBackend implementation (c1e469e)
- feat: add CacheBackend trait + filesystem implementation (f1ce847)

## [0.3.0] — 2026-04-24

- docs: update README with keygen, config, and segment documentation (caed159)
- feat: update post-build hook with segment routing (0b38d57)
- feat: add config file support with cache segments (fca441b)
- feat: add keygen command for 256-bit topic key generation (34796f3)
- feat: add .praxis foundation (ADR-0001, ADR-0002, milestone 0.3 expectations) (d53ef44)

## [0.2.2] — 2026-04-24

- fix: remove redundant double-write in NixOS post-build hook (3814f3b)
- fix: sync flake.nix version to 0.2.1 to match Cargo.toml (2025114)
- fix: unify license as MIT across all files (311063c)
- fix: stream NAR files instead of loading into memory (a9ae89c)
- docs: rewrite README to document actual CLI and capabilities (a94d8eb)

## [0.2.1] — 2026-04-24

- fix: axum route panic — /{hash}.narinfo is invalid, use /{hash_narinfo} with suffix check (264ac80)
- docs: refresh ROADMAP.md with OASIS strategic alignment (e1e1334)
- ignore multicast test — requires real network device, fails in nix sandbox (cbf7148)
- chore: change license to MIT in flake.nix (0c0bbaf)
- docs: update copilot-instructions with praxis, design-dojo, automation rules (8efef46)

# Changelog

## [0.2.0] — 2026-04-23

- feat(release): add target_version input for milestone-driven releases (2ffa43b)
- feat(lifecycle): milestone-close triggers roadmap-aware release (b364246)
- docs: update copilot-instructions with Plures stack architecture (8da59f4)
- docs: update copilot-instructions with Plures stack architecture (bd24bd5)
- feat(lifecycle v12): auto-release when milestone completes (2e17118)
- fix: resolve CI check failures from formatting drift and hook script mismatch (#7) (74d5777)
- feat: integrate first-class Nix post-build hook for automatic cache imports (#5) (096ee9c)
- feat(lifecycle v11): smart CI failure handling — infra vs code (a6a3f80)
- fix(lifecycle): label-based retry counter + CI fix priority (0df3d9d)
- feat: Hyperswarm P2P cache replication (#4) (af56552)
- style: fmt (e7e51c3)
- fix: clippy redundant closure (69f7bac)
- style: cargo fmt (b403e10)
- fix: separate CI from release workflow (9e048f3)
- feat: implement MVP Nix binary cache substituter (d9877ec)
- ci: inline lifecycle workflow — fix schedule failures (54717e3)
- ci: centralize lifecycle — event-driven with schedule guard (65397f9)
- fix(lifecycle): v9.2 — process all PRs per tick (return→continue), widen bot filter (637721a)
- fix(lifecycle): change return→continue so all PRs process in one tick (d41f698)
- fix(lifecycle): v9.1 — fix QA dispatch (client_payload as JSON object) (4e640a7)
- fix(lifecycle): rewrite v9 — apply suggestions, merge, no nudges (0c18f32)
- chore: standardize license to MIT (f37756c)
- chore: standardize copilot-pr-lifecycle.yml to canonical version (9e09275)
- fix: add packages:write + id-token:write to release workflow (490b939)
- docs: add ROADMAP.md (9c7edd0)
- Merge pull request #1 from plures/chore/org-standards (4dfb9fa)
- Update .github/workflows/copilot-pr-lifecycle.yml (4394bdc)
- Update .github/workflows/copilot-pr-lifecycle.yml (a924963)
- Update .github/workflows/copilot-pr-lifecycle.yml (6e7a6ef)
- Update .github/workflows/copilot-pr-lifecycle.yml (effd322)
- Update .github/workflows/release.yml (660b869)
- Update .github/workflows/tech-doc-writer.yml (d2dd6e1)
- Update .github/workflows/tech-doc-writer.yml (f55e436)
- Update .github/workflows/tech-doc-writer.yml (bd860f8)
- chore: add Reusable release pipeline (bd6001a)
- chore: add Auto-create doc issues on PR merge (f052bbc)
- chore: add Copilot PR auto-merge lifecycle (c512542)
- chore: add Copilot coding instructions (d443734)
- ci: add PR lane event relay to centralized merge FSM (eb5caa7)
- docs: formalize design docs and architecture (8367d88)
- chore: initial scaffold (fa5e3bb)
- Initial commit (0ec5027)

