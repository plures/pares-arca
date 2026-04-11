#!/usr/bin/env bash
# post-build-hook.sh — Nix post-build hook that auto-imports into Pares Arca.
#
# Install: Add to /etc/nix/nix.conf:
#   post-build-hook = /path/to/post-build-hook.sh
#
# Nix calls this with $OUT_PATHS (newline-separated store paths).

set -euo pipefail

CACHE_DIR="${PARES_CACHE_DIR:-$HOME/.cache/pares-arca}"
LOG="/tmp/pares-arca-hook.log"

# Ensure cache dir exists
mkdir -p "$CACHE_DIR/nar"

for path in $OUT_PATHS; do
    hash=$(basename "$path" | cut -d'-' -f1)
    
    # Skip if already cached
    if [ -f "$CACHE_DIR/$hash.narinfo" ]; then
        continue
    fi

    echo "[$(date -Iseconds)] Caching: $path" >> "$LOG"
    
    # Use pares-cache CLI if available, fall back to manual
    if command -v pares-cache &>/dev/null; then
        pares-cache import "$path" 2>>"$LOG" || true
    fi
done
