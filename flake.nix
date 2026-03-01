{
  description = "rind flake";

  inputs = {
    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs = { self, nixpkgs, flake-utils }:
    flake-utils.lib.eachDefaultSystem (system:
      let
        pkgs = import nixpkgs { inherit system; };

        builder = pkgs.rustPlatform.buildRustPackage {
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
          ];
        };
      }
    );
}
