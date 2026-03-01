{
  description = "rind flake";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
    rust-overlay.url = "github:oxalica/rust-overlay";
  };

  outputs = { self, nixpkgs, flake-utils, rust-overlay }:
    flake-utils.lib.eachDefaultSystem (system:
      let
        overlays = [ (import rust-overlay) ];
        pkgs = import nixpkgs { inherit system overlays; };

        rustToolchain = pkgs.rust-bin.stable.latest.default.override {
          targets = [ "x86_64-unknown-linux-musl" ];
        };

        rustPlatform = pkgs.makeRustPlatform {
          cargo = rustToolchain;
          rustc = rustToolchain;
        };

        builder = rustPlatform.buildRustPackage {
          pname = "builder";
          version = "0.1.0";

          src = ./builder;

          cargoLock = {
            lockFile = ./builder/Cargo.lock;
          };

          nativeBuildInputs = [
              pkgs.pkg-config
            ];

	        buildInputs = [
	          pkgs.openssl
	        ];

        };
      in
      {
        packages.default = builder;

        apps.default = {
          type = "app";
          program = "${builder}/bin/builder";
        };

        devShells.default = pkgs.mkShell {
          buildInputs = [
            builder
            rustToolchain
            pkgs.pkg-config
            pkgs.openssl
          ];
        };
      }
    );
}
