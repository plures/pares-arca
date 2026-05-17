#!/usr/bin/env bash
# post-build-hook.sh — Nix post-build hook with segment routing.
#
# Install: Add to /etc/nix/nix.conf:
#   post-build-hook = /path/to/post-build-hook.sh
#
# Nix calls this with $OUT_PATHS (newline-separated store paths).
# This hook routes each path to the correct cache segment:
#   - nixpkgs paths → universal segment
#   - custom paths → custom segment

set -euo pipefail

CONFIG_FILE="${PARES_ARCA_CONFIG:-${XDG_CONFIG_HOME:-$HOME/.config}/pares-arca/config.toml}"
CACHE_DIR="${PARES_ARCA_DIR:-$HOME/.cache/pares-arca}"
LOG="/tmp/pares-arca-hook.log"

# Determine if a store path is from nixpkgs.
# Uses nix path-info --json when available for accurate detection,
# falls back to name heuristics.
is_nixpkgs_path() {
    local path="$1"
    local basename
    basename=$(basename "$path")
    # Strip the 32-char hash prefix + dash
    local name_part="${basename:33}"

    # Heuristic: custom builds often start with "source" or contain "-custom-"/"-local-"
    if [[ "$name_part" == source* ]] || [[ "$name_part" == *-custom-* ]] || [[ "$name_part" == *-local-* ]]; then
        return 1  # not nixpkgs
    fi

    # Try nix path-info for more accurate detection
    if command -v nix &>/dev/null; then
        local info
        if info=$(nix path-info --json "$path" 2>/dev/null); then
            # If the deriver is from nixpkgs, it's a nixpkgs path
            if echo "$info" | grep -q '"nixpkgs"'; then
                return 0
            fi
        fi
    fi

    # Default: treat as nixpkgs (most builds are)
    return 0
}

# Determine segment for a path
get_segment() {
    local path="$1"
    if is_nixpkgs_path "$path"; then
        echo "universal"
    else
        echo "custom"
    fi
}

if [ -z "${OUT_PATHS:-}" ]; then
    exit 0
fi

while IFS= read -r path; do
    [ -z "$path" ] && continue

    segment=$(get_segment "$path")
    echo "[$(date -Iseconds)] Caching: $path → segment=$segment" >> "$LOG"

    if command -v pares-arca &>/dev/null; then
        PARES_ARCA_DIR="$CACHE_DIR" pares-arca import --segment "$segment" "$path" >/dev/null 2>&1 || true
    fi
done <<< "$OUT_PATHS"
