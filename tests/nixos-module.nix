# Evaluates the NixOS module as a real host configuration and verifies that
# enabling Arca adds (rather than replaces) its local Nix substituter.
{ arca ? builtins.getFlake (toString ../.) }:
let
  nixpkgs = arca.inputs.nixpkgs;
  cfg = (nixpkgs.lib.nixosSystem {
    system = "x86_64-linux";
    modules = [
      arca.nixosModules.default
      ({ ... }: {
        services.pares-arca.enable = true;
        networking.hostName = "arca-module-test";
      })
    ];
  }).config;
in
assert cfg.nix.settings.extra-substituters == [ "http://localhost:5555" ];
assert cfg.nix.settings.trusted-substituters == [ "http://localhost:5555" ];
assert cfg.systemd.services.pares-arca.serviceConfig.Environment
  == [
    "PARES_ARCA_DIR=/var/cache/pares-arca"
    "RUST_LOG=arca_server=info,arca_core=info,pares_arca=info"
  ];
true
