{
  description = "Pares Arca — distributed Nix binary cache with P2P sync";

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
          # Resolve effective signing key: explicit secretKeyFile > auto-generated > none
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
            # Import paths into cache — sign during import so narinfos are served with signatures
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
              default = "127.0.0.1";
              description = "Address to bind the HTTP server";
            };

            openFirewall = lib.mkOption {
              type = lib.types.bool;
              default = false;
              description = "Whether to open the firewall for the cache port";
            };

            postBuildHook = lib.mkOption {
              type = lib.types.bool;
              default = true;
              description = "Whether to auto-import Nix build outputs into Pares Arca via a post-build hook";
            };

            autoSigningKey = lib.mkOption {
              type = lib.types.bool;
              default = true;
              description = "Automatically generate a signing key pair on first start if none exists. Sets secretKeyFile and trusted-public-keys automatically.";
            };

            logLevel = lib.mkOption {
              type = lib.types.enum [ "error" "warn" "info" "debug" "trace" ];
              default = "info";
              description = "Log level for the pares-arca server. Set to 'debug' for troubleshooting.";
            };

            signingKeyDir = lib.mkOption {
              type = lib.types.path;
              default = "/var/lib/pares-arca/signing";
              description = "Directory to store the auto-generated signing key pair.";
            };
          };

          config = lib.mkIf cfg.enable (
            let
              # Resolve the effective secret key path: explicit > auto-generated
              effectiveSecretKey =
                if cfg.secretKeyFile != null then cfg.secretKeyFile
                else if cfg.autoSigningKey then "${cfg.signingKeyDir}/secret-key.pem"
                else null;
              effectivePublicKeyFile =
                if cfg.autoSigningKey && cfg.secretKeyFile == null
                then "${cfg.signingKeyDir}/public-key.pem"
                else null;
              hostname = config.networking.hostName or "pares-arca";
            in {
            # Auto-generate signing key pair on first activation
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
                chmod 755 "$KEY_DIR"  # Directory must be traversable by all users
                echo "[pares-arca] Signing key generated. Public key:"
                cat "$PUBLIC"
              fi
            '';

            systemd.services.pares-arca = {
              description = "Pares Arca — Nix binary cache";
              after = [ "network.target" ];
              wantedBy = [ "multi-user.target" ];

              serviceConfig = {
                ExecStartPre = lib.optionals (effectiveSecretKeyPath != null) [
                  "+${pkgs.writeShellScript "pares-arca-sign" ''
                    exec ${self.packages.${pkgs.system}.default}/bin/pares-arca sign --key-file ${effectiveSecretKeyPath}
                  ''}"
                ];
                ExecStart = "${self.packages.${pkgs.system}.default}/bin/pares-arca serve --bind ${cfg.bind}:${toString cfg.port}";
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

            networking.firewall.allowedTCPPorts =
              lib.optional cfg.openFirewall cfg.port;

            # Auto-configure Nix to use local cache
            # Always use localhost for the client URL — bind address (e.g. 0.0.0.0)
            # is for the server socket, not the client connection.
            nix.settings = {
              substituters = [ "http://localhost:${toString cfg.port}" ];
              trusted-substituters = [ "http://localhost:${toString cfg.port}" ];
            };

            # post-build-hook is a top-level nix option, not under settings
            # Merge with trust-key include into a single extraOptions block
            nix.extraOptions = lib.mkMerge [
              (lib.mkIf cfg.postBuildHook ''
                post-build-hook = ${postBuildHookScript}
              '')
              (lib.mkIf (cfg.autoSigningKey && cfg.secretKeyFile == null) ''
                !include ${cfg.signingKeyDir}/nix-trusted-key.conf
              '')
            ];

            # Generate a nix config snippet alongside the key pair
            system.activationScripts.pares-arca-trust-key = lib.mkIf (cfg.autoSigningKey && cfg.secretKeyFile == null) (lib.stringAfter [ "pares-arca-signing-key" ] ''
              KEY_DIR="${cfg.signingKeyDir}"
              PUBLIC="$KEY_DIR/public-key.pem"
              CONF="$KEY_DIR/nix-trusted-key.conf"
              if [ -f "$PUBLIC" ]; then
                PUB_KEY=$(cat "$PUBLIC")
                echo "trusted-public-keys = $PUB_KEY cache.nixos.org-1:6NCHdD59X431o0gWypbMrAURkbJ16ZPMQFGspcDShjY=" > "$CONF"
                # Public keys + trust config must be world-readable.
                # The nix client runs as the calling user and parses !include directives.
                # Secret key stays 600. Directory must be traversable (755).
                chmod 755 "$KEY_DIR"
                chmod 644 "$PUBLIC"
                chmod 644 "$CONF"
                echo "[pares-arca] Nix trusts local cache key: $PUB_KEY"
              fi
            '');
          });
        };
    };
}
