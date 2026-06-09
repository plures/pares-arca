{
  description = "Pares Arca - distributed Nix binary cache with P2P sync";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs = { self, nixpkgs, flake-utils, rust-overlay }:
    let
      # Read version from Cargo.toml
      cargoVersion = let
        cargo = builtins.readFile ./Cargo.toml;
        lines = builtins.filter (l: builtins.match ''version = ".*"'' l != null)
          (nixpkgs.lib.splitString "\n" cargo);
        raw = builtins.head lines;
      in builtins.head (builtins.match ''.*"(.*)".*'' raw);
    in
    flake-utils.lib.eachDefaultSystem (system:
      let
        overlays = [ (import rust-overlay) ];
        pkgs = import nixpkgs { inherit system overlays; };
        rust = pkgs.rust-bin.stable.latest.default.override {
          extensions = [ "rust-src" ];
        };
      in
      {
        packages.default = pkgs.rustPlatform.buildRustPackage {
          pname = "pares-arca";
          version = cargoVersion;
          src = pkgs.lib.cleanSource ./.;
          cargoLock = {
            lockFile = ./Cargo.lock;
            allowBuiltinFetchGit = true;
          };
          __noChroot = true;
          nativeBuildInputs = with pkgs; [ pkg-config ];
          buildInputs = with pkgs; [ openssl xz ];
          meta = {
            description = "Distributed Nix binary cache";
            homepage = "https://github.com/plures/pares-arca";
            license = pkgs.lib.licenses.mit;
            mainProgram = "pares-arca";
          };
        };

        devShells.default = pkgs.mkShell {
          buildInputs = with pkgs; [
            rust
            pkg-config
            openssl
            xz
            cargo-watch
          ];
        };
      }
    ) // {
      # NixOS module for running Arca as a service
      nixosModules.default = { config, lib, pkgs, ... }:
        let
          cfg = config.services.pares-arca;
          # Resolve effective signing key path
          effectiveSecretKeyPath =
            if cfg.secretKeyFile != null then cfg.secretKeyFile
            else if cfg.autoSigningKey then "${cfg.signingKeyDir}/secret-key.pem"
            else null;
          postBuildHookScript = pkgs.writeShellScript "pares-arca-post-build-hook" ''
            set -euo pipefail

            if [ -z "''${OUT_PATHS:-}" ]; then
              exit 0
            fi

            set -f
            # Import paths into cache, always signing (signing is cheap ed25519)
            ${lib.optionalString (effectiveSecretKeyPath != null) ''
            SIGN_ARG="--signing-key ${effectiveSecretKeyPath}"
            ''}
            ${lib.optionalString (effectiveSecretKeyPath == null) ''
            SIGN_ARG=""
            ''}
            for path in $OUT_PATHS; do
              PARES_ARCA_DIR=${lib.escapeShellArg (toString cfg.cacheDir)} ${self.packages.${pkgs.system}.default}/bin/pares-arca import $SIGN_ARG "$path" >/dev/null 2>&1 || true
            done
          '';
        in
        {
          options.services.pares-arca = {
            enable = lib.mkEnableOption "Pares Arca binary cache service";

            secretKeyFile = lib.mkOption {
              type = lib.types.nullOr lib.types.path;
              default = null;
              description = "Path to the Nix binary cache signing secret key. Generate with: nix-store --generate-binary-cache-key <name> secret-key public-key";
            };

            cacheDir = lib.mkOption {
              type = lib.types.path;
              default = "/var/cache/pares-arca";
              description = "Directory to store cached NARs";
            };

            port = lib.mkOption {
              type = lib.types.port;
              default = 5555;
              description = "Port to serve the binary cache on";
            };

            bind = lib.mkOption {
              type = lib.types.str;
              default = "0.0.0.0";
              description = "Address to bind the HTTP server. Defaults to all interfaces for LAN sharing.";
            };

            openFirewall = lib.mkOption {
              type = lib.types.bool;
              default = true;
              description = "Whether to open the firewall for the cache port. Defaults to true for LAN sharing.";
            };

            postBuildHook = lib.mkOption {
              type = lib.types.bool;
              default = true;
              description = "Whether to auto-import Nix build outputs into Pares Arca via a post-build hook";
            };

            autoSigningKey = lib.mkOption {
              type = lib.types.bool;
              default = true;
              description = "Automatically generate a signing key pair on first start if none exists.";
            };

            logLevel = lib.mkOption {
              type = lib.types.enum [ "error" "warn" "info" "debug" "trace" ];
              default = "info";
              description = "Log level for the pares-arca server.";
            };

            signingKeyDir = lib.mkOption {
              type = lib.types.path;
              default = "/var/lib/pares-arca/signing";
              description = "Directory to store the auto-generated signing key pair.";
            };

            sync = {
              enable = lib.mkOption {
                type = lib.types.bool;
                default = true;
                description = "Whether to enable P2P sync via PluresDB Hyperswarm. Enabled by default — all arca nodes share narinfo metadata with peers on the same topic.";
              };

              publicTopic = lib.mkOption {
                type = lib.types.str;
                default = "pares-arca-nixos-public";
                description = ''Hyperswarm topic for sharing official nixos.org
                  package narinfos. All arca nodes join this topic by default,
                  forming a global P2P cache network for packages fetched from
                  cache.nixos.org. Set to "" to disable the public topic
                  (e.g., air-gapped or corporate networks).'';
              };

              extraTopics = lib.mkOption {
                type = lib.types.listOf lib.types.str;
                default = [];
                description = ''Additional private sync topics. Use for
                  team/org caches, personal packages, or internal-only sharing.
                  Each topic forms an independent P2P swarm.'';
                example = [ "myteam-private-cache" "corp-internal-builds" ];
              };
            };
          };

          config = lib.mkIf cfg.enable (let
            hostname = config.networking.hostName or "pares-arca";
          in {
            # ── Key Generation ────────────────────────────────────────────
            # Generate signing key on first activation. Runs before nix-daemon
            # restarts during nixos-rebuild switch, so the key exists by the
            # time the daemon reads secret-key-files.
            system.activationScripts.pares-arca-signing-key = lib.mkIf cfg.autoSigningKey ''
              KEY_DIR="${cfg.signingKeyDir}"
              SECRET="$KEY_DIR/secret-key.pem"
              PUBLIC="$KEY_DIR/public-key.pem"
              if [ ! -f "$SECRET" ]; then
                echo "[pares-arca] Generating signing key pair in $KEY_DIR..."
                mkdir -p "$KEY_DIR"
                ${pkgs.nix}/bin/nix-store --generate-binary-cache-key \
                  "${hostname}-pares-arca-1" "$SECRET" "$PUBLIC"
                chmod 600 "$SECRET"
                chmod 644 "$PUBLIC"
                chmod 755 "$KEY_DIR"
                echo "[pares-arca] Signing key generated. Public key:"
                cat "$PUBLIC"
              fi
            '';

            # ── Nix Daemon Configuration ──────────────────────────────────
            # secret-key-files tells the nix daemon: "I own these keys, trust
            # paths signed by their public counterparts." This is the mechanism
            # designed for local binary caches — no !include, no manual
            # trusted-public-keys, no timing issues.
            #
            # From the Nix manual: "A trusted key is one listed in
            # trusted-public-keys, or a public key counterpart to a private
            # key stored in a file listed in secret-key-files."
            nix.settings = {
              substituters = [ "http://localhost:${toString cfg.port}" ];
              trusted-substituters = [ "http://localhost:${toString cfg.port}" ];
              secret-key-files = lib.optional (effectiveSecretKeyPath != null) effectiveSecretKeyPath;
            };

            # post-build-hook imports every build into the local cache
            nix.extraOptions = lib.mkIf cfg.postBuildHook ''
              post-build-hook = ${postBuildHookScript}
            '';

            # ── Service ───────────────────────────────────────────────────
            systemd.services.pares-arca = {
              description = "Pares Arca - Nix binary cache";
              after = [ "network.target" ];
              wantedBy = [ "multi-user.target" ];

              serviceConfig = {
                ExecStartPre = lib.optionals (effectiveSecretKeyPath != null) [
                  "+${pkgs.writeShellScript "pares-arca-sign" ''
                    exec ${self.packages.${pkgs.system}.default}/bin/pares-arca sign --key-file ${effectiveSecretKeyPath}
                  ''}"
                ];
                ExecStart = let
                  # Build list of all sync topics: public (if set) + extras
                  allTopics =
                    (lib.optional (cfg.sync.enable && cfg.sync.publicTopic != "") cfg.sync.publicTopic)
                    ++ (lib.optionals cfg.sync.enable cfg.sync.extraTopics);
                  syncArgs = lib.concatMapStrings (t: " --sync-topic ${t}") allTopics;
                in "${self.packages.${pkgs.system}.default}/bin/pares-arca serve --bind ${cfg.bind}:${toString cfg.port}${syncArgs}";
                DynamicUser = true;
                CacheDirectory = "pares-arca";
                StateDirectory = "pares-arca";
                Environment = [
                  "PARES_ARCA_DIR=${cfg.cacheDir}"
                  "RUST_LOG=arca_server=${cfg.logLevel},arca_core=${cfg.logLevel},pares_arca=${cfg.logLevel}"
                ];
                Restart = "on-failure";
                RestartSec = "5s";
              };
            };

            # ── Firewall ──────────────────────────────────────────────────
            networking.firewall.allowedTCPPorts =
              lib.optional cfg.openFirewall cfg.port;
          });
        };
    };
}
