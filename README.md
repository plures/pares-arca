# Pares Arca

> *arca* — "strongbox"

A peer-to-peer Nix binary cache with **segmented caching**, **narinfo signing**, and **zstd compression**. Import store paths locally, serve them over HTTP as a Nix substituter, and replicate narinfo metadata to peers via encrypted connections with UDP discovery.

## Key Concepts

### Cache Segments

Pares Arca organizes cached paths into **segments**, each with its own topic key for P2P replication:

- **Universal** — nixpkgs and well-known public packages. Ships with a well-known topic key so all users share the same public cache by default.
- **Custom** — your own packages, private builds, or team-specific derivations. Requires a unique topic key generated with `pares-arca keygen`.

Segments are defined in `~/.config/pares-arca/config.toml` (created automatically on first run).

## Installation

### Nix Flake

```bash
nix profile install github:plures/pares-arca
```

### From Source

```bash
git clone https://github.com/plures/pares-arca.git
cd pares-arca
cargo build --release
# Binary: target/release/pares-arca
```

### NixOS Module

```nix
{
  inputs.pares-arca.url = "github:plures/pares-arca";

  # In your configuration:
  imports = [ pares-arca.nixosModules.default ];

  services.pares-arca = {
    enable = true;
    port = 5555;
    postBuildHook = true;  # auto-import build outputs
  };
}
```

The module starts a systemd service, configures Nix to use the local cache as a substituter, and optionally installs a post-build hook that imports every build output automatically.

## Quick Start

```bash
# Import a store path (default: zstd compression)
pares-arca import /nix/store/abc123-hello-2.12

# Import with xz compression instead
pares-arca import --compression xz /nix/store/abc123-hello-2.12

# Import and sign with ed25519 key
pares-arca import --signing-key ~/.config/pares-arca/my-cache.secret /nix/store/abc123-hello-2.12

# Import an entire flake closure
pares-arca import-closure .

# List cached paths
pares-arca list

# Check cache status (includes dedup stats)
pares-arca status

# Serve as a Nix substituter (HTTP)
pares-arca serve --bind 127.0.0.1:5555

# Install the Nix post-build hook (auto-import all builds)
sudo pares-arca install-hook

# Generate a topic key for a custom/private segment
pares-arca keygen

# Generate an ed25519 signing keypair
pares-arca sign-keygen --name my-cache

# Garbage collect old entries
pares-arca gc --max-age 30d

# Garbage collect to fit under a size limit
pares-arca gc --max-size 10G
```

## Narinfo Signing

Nix verifies narinfo signatures to ensure cache integrity. Pares Arca supports ed25519 signing:

```bash
# Generate a keypair
pares-arca sign-keygen --name my-cache
# Output:
#   Secret key: ~/.config/pares-arca/my-cache.secret
#   Public key: ~/.config/pares-arca/my-cache.pub
#   Public key: my-cache:Base64PublicKey==

# Import with signing
pares-arca import --signing-key ~/.config/pares-arca/my-cache.secret /nix/store/...

# Or set in config.toml:
# signing_key_path = "/home/user/.config/pares-arca/my-cache.secret"
```

Add the public key to consumers' `nix.conf`:
```
trusted-public-keys = cache.nixos.org-1:... my-cache:Base64PublicKey==
```

## Compression

Pares Arca supports **zstd** (default) and **xz** compression for NAR archives:

| Algorithm | Extension | Speed | Ratio | Default |
|-----------|-----------|-------|-------|---------|
| zstd      | `.nar.zst` | Fast (level 3) | Good | ✅ |
| xz        | `.nar.xz`  | Slow (level 6) | Best | — |

```bash
# Use zstd (default)
pares-arca import /nix/store/...

# Use xz for better compression ratio
pares-arca import --compression xz /nix/store/...
```

Set default in `config.toml`:
```toml
compression = "zstd"  # or "xz"
```

The HTTP server automatically detects the compression format from the file extension and sets the correct `Content-Type` header (`application/zstd` or `application/x-xz`).

## Garbage Collection

Remove old or excess cache entries:

```bash
# Remove entries older than 30 days
pares-arca gc --max-age 30d

# Remove LRU entries to fit under 10 GB
pares-arca gc --max-size 10G

# Both at once
pares-arca gc --max-age 7d --max-size 5G
```

Duration formats: `30d`, `7d`, `24h`, `1h30m`
Size formats: `10G`, `500M`, `1T`, `100K`

GC removes both narinfo metadata and NAR data from the object store, and records `GarbageCollect` events in the audit log.

### Configuration

The config file at `~/.config/pares-arca/config.toml` defines your cache segments:

```toml
compression = "zstd"
# signing_key_path = "/home/user/.config/pares-arca/my-cache.secret"

[[segments]]
name = "universal"
topic_key = "a1b2c3d4e5f6a7b8c9d0e1f2a3b4c5d6e7f8a9b0c1d2e3f4a5b6c7d8e9f0a1b2"
description = "Public nixpkgs binary cache"
filter = "nixpkgs"

[[segments]]
name = "team"
topic_key = "<output of pares-arca keygen>"
description = "Our team's private packages"
filter = "custom"
```

**Filters:**
- `nixpkgs` — matches store paths from nixpkgs
- `custom` — matches non-nixpkgs paths (custom builds, flake outputs)
- `all` — matches everything

### Two-Machine Setup Example

```bash
# On both machines:
pares-arca keygen  # Share this key securely
pares-arca sign-keygen --name team-cache  # Share public key

# Edit ~/.config/pares-arca/config.toml on both machines:
# Add a "team" segment with the shared key and filter = "custom"
# Set signing_key_path to the secret key

# Machine A:
pares-arca swarm --also-serve

# Machine B:
pares-arca swarm --also-serve --static-peer <machine-a-ip>:7070
```

Then add to your `nix.conf`:

```
substituters = http://127.0.0.1:5555
trusted-public-keys = team-cache:Base64PublicKey==
```

## P2P Swarm Sync

Start a swarm node to replicate cached narinfo metadata with peers:

```bash
pares-arca swarm --topic "my-team-key"
```

All nodes sharing the same `--topic` string discover each other via UDP multicast, connect over Noise XX encrypted TCP, and exchange narinfo records. Only the SHA-256 hash of the topic is sent on the wire — the raw string stays private.

```bash
# Full options
pares-arca swarm \
  --topic "my-team-key" \
  --discovery-port 7070 \
  --sync-port 7071 \
  --http-port 5555 \
  --also-serve \
  --static-peer 10.0.0.1:7070
```

- `--also-serve` starts the HTTP substituter alongside the swarm
- `--static-peer` adds bootstrap peers for networks without multicast

### How Sync Works

1. Your node announces on the topic via UDP discovery
2. Peers with the same topic connect over Noise-encrypted TCP
3. Narinfo metadata is exchanged — peers learn what paths you have
4. NAR files are fetched on-demand over HTTP when Nix requests them

## CLI Reference

| Command | Description |
|---|---|
| `serve --bind <addr>` | Start HTTP substituter (default: `127.0.0.1:5555`) |
| `import <store-path>` | Import a single Nix store path (auto-routes to segment) |
| `import --compression zstd\|xz` | Import with specific compression (default: zstd) |
| `import --signing-key <path>` | Import and sign with ed25519 key |
| `import --segment <name> <path>` | Import to a specific segment |
| `import-closure <flake-ref>` | Import all paths in a flake closure |
| `status` | Show cache directory, path count, size, dedup stats |
| `list` | List all cached store paths |
| `swarm` | Start P2P discovery and narinfo replication |
| `install-hook` | Install Nix post-build hook at `/etc/nix/post-build-hook` |
| `keygen` | Generate a 256-bit random topic key |
| `sign-keygen --name <name>` | Generate ed25519 signing keypair |
| `gc --max-age <dur>` | Remove entries older than duration |
| `gc --max-size <size>` | Remove LRU entries to fit under size limit |

Global options:
- `--cache-dir <path>` or `PARES_ARCA_DIR` env var (default: `~/.cache/pares-arca`)
- `--backend filesystem|sled` — storage backend (default: `sled`)
- `--db-path <path>` — sled database directory (default: `<cache-dir>/db`)

## Architecture

Four crates:

| Crate | Role |
|---|---|
| `arca-core` | Cache store, narinfo parsing, config, segment routing, signing, compression, GC, import/export, backend trait, sled store, audit log |
| `arca-server` | Axum HTTP server implementing the Nix binary cache protocol |
| `arca-swarm` | UDP discovery, Noise XX transport, narinfo CRDT sync, topic management |
| `arca-cli` | CLI (`pares-arca` binary) wiring everything together |

### Storage Layout

```
~/.cache/pares-arca/
├── nix-cache-info       # Cache metadata
├── <hash>.narinfo       # Per-path narinfo files (filesystem backend)
├── db/                  # Sled database (sled backend)
├── objects/
│   ├── chunks/          # Content-addressed chunks (plures-object)
│   └── manifests/       # Object manifests
└── nar/
    └── <hash>.nar.zst   # Compressed NAR archives (legacy fallback)
```

### Storage Backends

- **sled** (default) — narinfo metadata in an embedded [sled](https://docs.rs/sled) database. Fast, atomic, foundation for PluresDB integration.
- **filesystem** — narinfo as flat files, NARs in `nar/` subdirectory.

```bash
# Use filesystem backend
pares-arca --backend filesystem status
```

### Content-Addressed Storage

NAR blobs are stored through [plures-object](https://github.com/plures/plures-object), which provides:

- **Deduplication** — identical chunks across different NARs are stored once
- **Content addressing** — SHA-256 chunk IDs guarantee integrity
- **Streaming retrieval** — NARs are reassembled from chunks on demand

### Audit Log

When using the sled backend, an append-only audit log records cache events (Import, SyncReceive, Serve, GarbageCollect) with timestamps for traceability.

## Current Limitations

- **Narinfo-only sync** — The swarm replicates narinfo metadata; NAR files are fetched over HTTP on demand.
- **LAN discovery** — UDP multicast works on local networks. Use `--static-peer` for cross-network sync.

## Part of Pares

Pares Arca is part of the [Pares](https://github.com/plures) ecosystem of P2P tools built on [Hyperswarm](https://docs.holepunch.to/) for discovery and encrypted connectivity.

## License

MIT — see [LICENSE](LICENSE).
