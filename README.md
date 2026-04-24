# Pares Arca

> *arca* — "strongbox"

A peer-to-peer Nix binary cache. Import store paths locally, serve them over HTTP as a Nix substituter, and replicate narinfo metadata to peers via encrypted Noise XX connections with UDP discovery.

## Installation

### Nix Flake

```bash
nix profile install github:plures/pares-cache
```

### From Source

```bash
git clone https://github.com/plures/pares-cache.git
cd pares-cache
cargo build --release
# Binary: target/release/pares-cache
```

### NixOS Module

```nix
{
  inputs.pares-cache.url = "github:plures/pares-cache";

  # In your configuration:
  imports = [ pares-cache.nixosModules.default ];

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
# Import a store path into the cache
pares-cache import /nix/store/abc123-hello-2.12

# Import an entire flake closure
pares-cache import-closure .

# List cached paths
pares-cache list

# Check cache status
pares-cache status

# Serve as a Nix substituter (HTTP)
pares-cache serve --bind 127.0.0.1:5555

# Install the Nix post-build hook (auto-import all builds)
sudo pares-cache install-hook
```

Then add to your `nix.conf`:

```
substituters = http://127.0.0.1:5555
trusted-substituters = http://127.0.0.1:5555
```

## P2P Swarm Sync

Start a swarm node to replicate cached narinfo metadata with peers:

```bash
pares-cache swarm --topic "my-team-key"
```

All nodes sharing the same `--topic` string discover each other via UDP multicast, connect over Noise XX encrypted TCP, and exchange narinfo records. Only the SHA-256 hash of the topic is sent on the wire — the raw string stays private.

```bash
# Full options
pares-cache swarm \
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
| `import <store-path>` | Import a single Nix store path |
| `import-closure <flake-ref>` | Import all paths in a flake closure |
| `status` | Show cache directory, path count, total size |
| `list` | List all cached store paths |
| `swarm` | Start P2P discovery and narinfo replication |
| `install-hook` | Install Nix post-build hook at `/etc/nix/post-build-hook` |

Global option: `--cache-dir <path>` or `PARES_CACHE_DIR` env var (default: `~/.cache/pares-arca`).

## Architecture

Four crates:

| Crate | Role |
|---|---|
| `arca-core` | Cache store, narinfo parsing, import/export, filesystem operations |
| `arca-server` | Axum HTTP server implementing the Nix binary cache protocol |
| `arca-swarm` | UDP discovery, Noise XX transport, narinfo CRDT sync, topic management |
| `arca-cli` | CLI (`pares-cache` binary) wiring everything together |

### Storage Layout

```
~/.cache/pares-arca/
├── nix-cache-info       # Cache metadata
├── <hash>.narinfo       # Per-path narinfo files
└── nar/
    └── <hash>.nar.xz    # Compressed NAR archives
```

## Current Limitations

- **No signing** — NARs are not cryptographically signed yet. Use `trusted-substituters` in nix.conf.
- **Narinfo-only sync** — The swarm replicates narinfo metadata; NAR files are fetched over HTTP on demand.
- **xz compression only** — Imported NARs are compressed with xz. No zstd support yet.
- **LAN discovery** — UDP multicast works on local networks. Use `--static-peer` for cross-network sync.

## Part of Pares

Pares Arca is part of the [Pares](https://github.com/plures) ecosystem of P2P tools built on [Hyperswarm](https://docs.holepunch.to/) for discovery and encrypted connectivity.

## License

MIT — see [LICENSE](LICENSE).
