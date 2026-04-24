{
  description = "Pares Arca — distributed Nix binary cache backed by PluresDB";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs = { self, nixpkgs, flake-utils, rust-overlay }:
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
          pname = "pares-cache";
          version = "0.1.0";
          src = ./.;
          cargoLock.lockFile = ./Cargo.lock;
          nativeBuildInputs = with pkgs; [ pkg-config ];
          buildInputs = with pkgs; [ openssl xz ];
          meta = {
            description = "Distributed Nix binary cache";
            homepage = "https://github.com/plures/pares-cache";
            license = pkgs.lib.licenses.mit;
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
          postBuildHookScript = pkgs.writeShellScript "pares-arca-post-build-hook" ''
            set -euo pipefail

            if [ -z "''${OUT_PATHS:-}" ]; then
              exit 0
            fi

            set -f
            for path in $OUT_PATHS; do
              PARES_CACHE_DIR=${lib.escapeShellArg (toString cfg.cacheDir)} ${self.packages.${pkgs.system}.default}/bin/pares-cache import "$path" >/dev/null 2>&1 || true
              PARES_CACHE_DIR=${cfg.cacheDir} ${self.packages.${pkgs.system}.default}/bin/pares-cache import "$path" >/dev/null 2>&1 || true
            done
          '';
        in
        {
          options.services.pares-arca = {
            enable = lib.mkEnableOption "Pares Arca binary cache service";

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
          };

          config = lib.mkIf cfg.enable {
            systemd.services.pares-arca = {
              description = "Pares Arca — Nix binary cache";
              after = [ "network.target" ];
              wantedBy = [ "multi-user.target" ];

              serviceConfig = {
                ExecStart = "${self.packages.${pkgs.system}.default}/bin/pares-cache serve --bind ${cfg.bind}:${toString cfg.port}";
                DynamicUser = true;
                CacheDirectory = "pares-arca";
                StateDirectory = "pares-arca";
                Environment = "PARES_CACHE_DIR=${cfg.cacheDir}";
                Restart = "on-failure";
                RestartSec = "5s";
              };
            };

            networking.firewall.allowedTCPPorts =
              lib.optional cfg.openFirewall cfg.port;

            # Auto-configure Nix to use local cache
            nix.settings = {
              substituters = [ "http://${cfg.bind}:${toString cfg.port}" ];
              trusted-substituters = [ "http://${cfg.bind}:${toString cfg.port}" ];
            } // lib.optionalAttrs cfg.postBuildHook {
              post-build-hook = postBuildHookScript;
            };
          };
        };
    };
}
