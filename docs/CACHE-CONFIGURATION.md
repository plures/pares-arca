# Pares Arca — Cache Configuration Guide

## Overview

Pares Arca runs as a local Nix binary cache with P2P replication. Out of the box it works with zero configuration — signing keys are auto-generated, the cache is local-only, and every build is automatically imported.

This guide covers three deployment scenarios:

1. **Default (local-only)** — caches your builds, speeds up rebuilds
2. **Public cache** — share packages with the community via the Pares mesh
3. **Private cache** — team or corporate builds that must stay confidential

---

## 1. Default: Local Cache (Zero Configuration)

### NixOS Flake

Add pares-cache as a flake input and enable the service:

```nix
# flake.nix
inputs.pares-cache.url = "github:plures/pares-cache";
inputs.pares-cache.inputs.nixpkgs.follows = "nixpkgs";
```

```nix
# configuration.nix
imports = [ inputs.pares-cache.nixosModules.default ];

services.pares-arca = {
  enable = true;
};
```

That's it. On first `nixos-rebuild`:

- A signing key pair is auto-generated at `/var/lib/pares-arca/signing/`
- Nix is auto-configured to use `http://127.0.0.1:5555` as a substituter
- The signing public key is auto-trusted
- A post-build hook auto-imports every build into the cache and signs it

Every subsequent `nix build` or `nixos-rebuild` checks the local cache first.

### Standalone (non-NixOS)

```bash
# Install
nix profile install github:plures/pares-cache

# Generate signing key
nix-store --generate-binary-cache-key $(hostname)-1 \
  ~/.config/pares-cache/secret-key.pem \
  ~/.config/pares-cache/public-key.pem

# Start the cache server
pares-cache serve --bind 127.0.0.1:5555 &

# Add to ~/.config/nix/nix.conf
cat >> ~/.config/nix/nix.conf <<EOF
substituters = http://127.0.0.1:5555 https://cache.nixos.org
trusted-public-keys = $(cat ~/.config/pares-cache/public-key.pem) cache.nixos.org-1:6NCHdD59X431o0gWypbMrAURkbJ16ZPMQFGspcDShjY=
post-build-hook = $(which pares-cache) import
EOF
```

### Verify it works

```bash
# Build something
nix build nixpkgs#hello

# Import it
pares-cache import $(nix build nixpkgs#hello --print-out-paths --no-link)

# Check the cache serves it
curl -s http://127.0.0.1:5555/nix-cache-info
# → StoreDir: /nix/store

# Check narinfo
HASH=$(nix build nixpkgs#hello --print-out-paths --no-link | xargs basename | cut -d- -f1)
curl -s http://127.0.0.1:5555/${HASH}.narinfo | head -5
# → StorePath: /nix/store/...
```

---

## 2. Public Cache — Share with the Pares Mesh

The public cache uses Pares Arca's built-in P2P swarm to replicate packages across the network. Anyone running the same segment topic key receives your cached builds automatically.

### Enable the swarm

```nix
# NixOS
services.pares-arca = {
  enable = true;
  bind = "0.0.0.0";       # Accept LAN connections
  openFirewall = true;     # Open ports 5555 (HTTP), 7070 (UDP discovery), 7071 (TCP sync)
};

# Add the swarm service (systemd unit)
systemd.services.pares-arca-swarm = {
  description = "Pares Arca Swarm — P2P cache replication";
  wantedBy = [ "multi-user.target" ];
  after = [ "pares-arca.service" "network-online.target" ];
  wants = [ "network-online.target" ];

  serviceConfig = {
    ExecStart = "${inputs.pares-cache.packages.${pkgs.system}.default}/bin/pares-cache swarm --topic plures-public-cache --discovery-port 7070 --sync-port 7071";
    Restart = "on-failure";
    RestartSec = 10;
  };
};

# Open swarm ports
networking.firewall = {
  allowedTCPPorts = [ 5555 7071 ];
  allowedUDPPorts = [ 7070 ];
};
```

### How it works

- The `--topic` flag determines which swarm you join
- `plures-public-cache` is the well-known public topic — all Pares Arca users share this
- Narinfo metadata replicates via UDP discovery + TCP sync
- NAR blobs are fetched on-demand when a peer requests a path you have
- **Only narinfo metadata is proactively replicated** — actual build outputs are fetched lazily

### Custom public segments

If you maintain a package set (e.g., a NUR repo) and want a dedicated public cache:

```bash
# Generate a topic key for your segment
pares-cache keygen --name "my-packages"
# → Topic key: a1b2c3d4...

# Share this topic key with your users
```

Users add it to their config:

```toml
# ~/.config/pares-cache/config.toml
[[segments]]
name = "my-packages"
topic = "a1b2c3d4..."
```

---

## 3. Private Cache — Teams and Organizations

Private caches keep builds confidential. Narinfo metadata and NAR blobs are only shared with peers that have the same topic key AND shared encryption key.

### Generate keys

```bash
# Topic key — identifies the swarm (who can discover peers)
pares-cache keygen --name "acme-corp"
# → Topic key: deadbeef...

# Shared encryption key — encrypts swarm traffic (who can read data)
pares-cache keygen --name "acme-corp-secret" --sea
# → SEA key: cafebabe...
```

### NixOS configuration

```nix
services.pares-arca = {
  enable = true;
  # Signing key is auto-generated
};

# Private swarm — encrypted with SEA key
systemd.services.pares-arca-swarm-private = {
  description = "Pares Arca Private Swarm — acme-corp";
  wantedBy = [ "multi-user.target" ];
  after = [ "pares-arca.service" "network-online.target" ];
  wants = [ "network-online.target" ];

  serviceConfig = {
    ExecStart = "${inputs.pares-cache.packages.${pkgs.system}.default}/bin/pares-cache swarm --topic deadbeef... --shared-key-file /run/agenix/acme-swarm-key --discovery-port 7080 --sync-port 7081";
    Restart = "on-failure";
    RestartSec = 10;
  };
};

# Different ports so public and private swarms can coexist
networking.firewall = {
  allowedTCPPorts = [ 7081 ];
  allowedUDPPorts = [ 7080 ];
};
```

### Distribute keys to your team

**Option A: Agenix (recommended for NixOS)**

Store the SEA key and topic key as agenix secrets:

```nix
age.secrets.acme-swarm-key = {
  file = ./secrets/acme-swarm-key.age;
  owner = "pares-arca";
  mode = "0400";
};
```

Encrypt with your team members' SSH keys:

```bash
agenix -e secrets/acme-swarm-key.age
# Paste the SEA key, save
```

**Option B: Environment variable**

```bash
export PARES_SYNC_SHARED_KEY="cafebabe..."
pares-cache swarm --topic deadbeef...
```

**Option C: Config file**

```toml
# ~/.config/pares-cache/config.toml
[[segments]]
name = "acme-corp"
topic = "deadbeef..."
shared_key = "cafebabe..."  # Or shared_key_file = "/path/to/key"
```

### Running public + private simultaneously

You can run multiple swarm instances on different ports:

```nix
# Public cache (shared with everyone)
systemd.services.pares-arca-swarm-public = {
  serviceConfig.ExecStart = "... swarm --topic plures-public-cache --discovery-port 7070 --sync-port 7071";
};

# Private cache (team only, encrypted)
systemd.services.pares-arca-swarm-private = {
  serviceConfig.ExecStart = "... swarm --topic deadbeef... --shared-key-file /run/agenix/key --discovery-port 7080 --sync-port 7081";
};
```

Both share the same local cache directory and HTTP server. The swarm determines what gets replicated to whom.

---

## Configuration Reference

### NixOS Module Options

| Option | Type | Default | Description |
|--------|------|---------|-------------|
| `enable` | bool | `false` | Enable the Pares Arca service |
| `port` | int | `5555` | HTTP server port |
| `bind` | string | `"127.0.0.1"` | Bind address (`0.0.0.0` for LAN) |
| `cacheDir` | path | `/var/cache/pares-arca` | Cache storage directory |
| `openFirewall` | bool | `false` | Open HTTP port in firewall |
| `postBuildHook` | bool | `true` | Auto-import builds into cache |
| `autoSigningKey` | bool | `true` | Auto-generate signing keys on first boot |
| `signingKeyDir` | path | `/var/lib/pares-arca/signing` | Directory for auto-generated keys |
| `secretKeyFile` | path | `null` | Explicit signing key (overrides auto) |

### Config File (`~/.config/pares-cache/config.toml`)

```toml
# Default segment — always present
[[segments]]
name = "universal"
topic = "plures-public-cache"

# Custom public segment
[[segments]]
name = "my-packages"
topic = "a1b2c3d4..."

# Private segment with encryption
[[segments]]
name = "acme-corp"
topic = "deadbeef..."
shared_key_file = "/path/to/sea-key"
```

### Environment Variables

| Variable | Description |
|----------|-------------|
| `PARES_CACHE_DIR` | Override cache directory |
| `PARES_SYNC_SHARED_KEY` | SEA encryption key for swarm |
| `PARES_NIX_HOST` | Hostname for signing key generation |

---

## Security Model

| Layer | Public Cache | Private Cache |
|-------|-------------|---------------|
| **Discovery** | Open (UDP multicast) | Topic key required |
| **Transport** | Unencrypted (Noise XX handshake) | SEA-encrypted |
| **Content** | Narinfo signed with cache key | Same + swarm encryption |
| **Trust** | Public key in `trusted-public-keys` | Same |

- **Signing keys** prove cache integrity — "this path was built by this cache"
- **Topic keys** control swarm membership — "who can discover peers"
- **SEA keys** encrypt swarm traffic — "who can read replicated data"
- **Nix trusted-public-keys** control substitution — "which caches does Nix trust"

A private cache with all three layers means: only team members can discover peers, only they can read traffic, and only signed paths are substituted.
