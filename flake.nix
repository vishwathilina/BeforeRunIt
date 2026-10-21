{
  description = "BeforeRunIt: live command risk line for Linux shells";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

  outputs = { self, nixpkgs }:
    let
      systems = [ "x86_64-linux" "aarch64-linux" ];
      forAll = f: nixpkgs.lib.genAttrs systems (system: f nixpkgs.legacyPackages.${system});
    in
    {
      packages = forAll (pkgs: rec {
        beforerunit = pkgs.rustPlatform.buildRustPackage {
          pname = "beforerunit";
          version = "0.1.0";
          src = ./.;
          cargoLock.lockFile = ./Cargo.lock;

          nativeBuildInputs = [ pkgs.pkg-config ];

          # Build the binary and the bash builtin (which needs unwinding panics).
          buildPhase = ''
            runHook preBuild
            cargo build --frozen --release
            cargo build --frozen --profile release-bash -p beforerunit-bash
            runHook postBuild
          '';
          checkPhase = ''
            runHook preCheck
            cargo test --frozen --workspace
            runHook postCheck
          '';
          installPhase = ''
            runHook preInstall
            install -Dm755 target/release/beforerunit $out/bin/beforerunit
            ln -s beforerunit $out/bin/bri
            install -Dm644 target/release-bash/libbeforerunit_bash.so $out/lib/beforerunit/libbeforerunit_bash.so
            runHook postInstall
          '';

          meta = with pkgs.lib; {
            description = "Live command risk line for Linux shells";
            homepage = "https://github.com/beforerunit/beforerunit";
            license = with licenses; [ mit asl20 ];
            mainProgram = "beforerunit";
            platforms = platforms.linux;
          };
        };
        default = beforerunit;
      });

      devShells = forAll (pkgs: {
        default = pkgs.mkShell {
          packages = with pkgs; [ cargo rustc clippy rustfmt rust-analyzer zsh fish ];
        };
      });
    };
}
