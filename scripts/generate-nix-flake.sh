#!/usr/bin/env bash
# Generates a standalone QxChat_vX.Y.Z_flake.nix for NixOS users.
# Called from the CI publish-release job.

set -euo pipefail

VERSION="${1:-}"
if [[ -z "$VERSION" ]]; then
  echo "Usage: $0 <version>" >&2
  exit 1
fi

REF="v${VERSION}"

cat <<FLAKE
# QxChat v${VERSION} — NixOS flake
# Drop this into your NixOS configuration to install QxChat.
#
# Usage (flake.nix):
#
#   {
#     inputs.qxchat.url = "path:./QxChat_v${VERSION}_flake.nix";
#     # or from the release asset directly:
#     # inputs.qxchat.url = "https://github.com/lqxp/app/releases/download/v${VERSION}/QxChat_v${VERSION}_flake.nix";
#   }
#
# Then in your NixOS module:
#
#   { inputs, ... }: {
#     imports = [ inputs.qxchat.nixosModules.default ];
#     nixpkgs.overlays = [ inputs.qxchat.overlays.default ];
#     programs.qxchat.enable = true;
#   }

{
  description = "QxChat v${VERSION} — NixOS flake";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

    qxchat-src = {
      url = "git+https://github.com/lqxp/app.git?ref=${REF}&submodules=1";
      flake = false;
    };
  };

  outputs =
    { self, nixpkgs, qxchat-src }:
    let
      systems = [
        "x86_64-linux"
        "aarch64-linux"
      ];

      forAllSystems = nixpkgs.lib.genAttrs systems;

      qxchatPackage =
        { system }:
        let
          pkgs = import nixpkgs {
            inherit system;
            config.allowUnfree = true;
          };
        in
        pkgs.callPackage "\${qxchat-src}/nix/qxchat.nix" { };
    in
    {
      nixosModules.default = import "\${qxchat-src}/nix/module.nix";

      overlays.default = final: prev: {
        qxchat = prev.callPackage "\${qxchat-src}/nix/qxchat.nix" { };
      };

      packages = forAllSystems (
        system:
        {
          default = qxchatPackage { inherit system; };
          qxchat = qxchatPackage { inherit system; };
        }
      );
    };
}
FLAKE
