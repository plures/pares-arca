# ADR-0002: Topic Key Security Model

## Status

Accepted — 2026-04-24

## Context

The current `arca-swarm` topic is a human-readable string (default: `pares-cache-default`) hashed with SHA-256 before use on the wire. This means anyone who guesses the topic string can join the swarm and read/write cache data. For a public nixpkgs cache this is acceptable, but for private/custom caches it is not.

## Decision

### Topic keys are 256-bit random

- Generated via `pares-cache keygen` using OS CSPRNG (`OsRng`)
- Output as 64-character hex string
- Stored in config file at `~/.config/pares-cache/config.toml`

### Universal nixpkgs topic uses a well-known key

- Convenience over secrecy for public packages — everyone shares the same nixpkgs
- The well-known key is published in the README and default config
- Users can remove the universal segment if they don't want public nixpkgs sharing

### Custom topics require explicitly generated keys

- `pares-cache keygen` generates a key
- User shares the key out-of-band (paste, secret manager, agenix)
- Config file stores topic+key pairs per segment

### PluresDB handles encryption

- Encryption at rest via SEA (PluresDB's built-in encryption)
- Authenticated replication via PluresDB's Hyperswarm topic-based access control
- No PSK in Noise handshake — PluresDB's topic key IS the access control

## Consequences

- `keygen` subcommand required in CLI
- Config file format must support multiple segments with topic keys
- Agenix integration documented for NixOS deployments (store topic keys as agenix secrets)
- The old `--topic <string>` CLI flag is deprecated in favor of config-file segments

## Evidence

| Fact | Source | Tested? |
|------|--------|---------|
| SHA-256 of guessable string is not a security boundary | Cryptographic first principles | N/A |
| `OsRng` uses OS CSPRNG (getrandom on Linux) | rand crate docs | Yes |
| PluresDB SEA encrypts data at rest | pluresdb docs | Yes |
| PluresDB Hyperswarm topic = access control boundary | pluresdb architecture | Yes |
