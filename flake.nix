# SPDX-FileCopyrightText: 2021 Serokell <https://serokell.io/>
#
# SPDX-License-Identifier: CC0-1.0
{
  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";
    flake-parts.url = "github:hercules-ci/flake-parts";
  };

  outputs =
    { nixpkgs, flake-parts, ... }@inputs:
    flake-parts.lib.mkFlake { inherit inputs; } {
      systems = [
        "x86_64-linux"
        "aarch64-darwin"
        "x86_64-darwin"
      ];
      perSystem =
        {
          config,
          self',
          inputs',
          pkgs,
          system,
          lib,
          ...
        }:
        {
          devShells.default =
            with pkgs;
            mkShell.override { stdenv = pkgs.clangStdenv; } {
              RUST_SRC_PATH = "${pkgs.rust.packages.stable.rustPlatform.rustLibSrc}";
              RUST_BACKTRACE = 1;

              buildInputs = [
                systemd
              ];
              nativeBuildInputs = [
                pkg-config
                nixfmt-rfc-style
                nixd
                rustc
                cargo
                rust-analyzer
                clippy
                openssl
                rustfmt
                pcsclite
                opensc
                cargo-generate
                worker-build
                wasm-pack
                lld
              ];
            };

          packages.dockerImage = pkgs.dockerTools.buildImage {
            name = "webrtc-signal-server";
            tag = "latest";
            copyToRoot = pkgs.buildEnv {
              name = "image-root";
              paths = [
                (pkgs.rustPlatform.buildRustPackage {
                  pname = "webrtc-signal-server";
                  version = "0.1.1";
                  src = ./.;
                  cargoLock = {
                    lockFile = ./Cargo.lock;
                  };
                  buildInputs = [ ];
                  nativeBuildInputs = [ ];
                  cargoBuildFlags = [
                    "-p"
                    "webrtc-signal-server"
                    "--bin"
                    "webrtc-signal-server"
                  ];
                })
              ];
              pathsToLink = [ "/bin" ];
            };
            config = {
              Cmd = [ "/bin/webrtc-signal-server" ];
              ExposedPorts = {
                "9000/tcp" = { };
              };
            };
          };
        };
    };
}
