{
  description = "projectwc - A Wayland compositor in Rust.";
  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
  };
  outputs = {
    self,
    nixpkgs,
  }: let
    systems = ["x86_64-linux" "aarch64-linux"];

    forAllSystems = fn: nixpkgs.lib.genAttrs systems (system: fn nixpkgs.legacyPackages.${system});
  in {
    packages = forAllSystems (pkgs: rec {
      default = pkgs.callPackage ./default.nix {
        gitRev = self.rev or self.dirtyRev or null;
      };
      projectwc = default;
    });

    devShells = forAllSystems (pkgs: {
      default = pkgs.mkShell {
        inputsFrom = [self.packages.${pkgs.stdenv.hostPlatform.system}.projectwc];
        packages = [
          pkgs.rustc
          pkgs.cargo
          pkgs.clippy
          pkgs.foot
          pkgs.westonLite # weston-terminal
          pkgs.just
          pkgs.pkg-config
        ];
        shellHook = ''
          export PS1="(projectwc-dev) $PS1"
        '';

        env = {
          RUST_SRC_PATH = pkgs.rustPlatform.rustLibSrc;
          LD_LIBRARY_PATH = pkgs.lib.makeLibraryPath [pkgs.wayland pkgs.libGL];
        };
      };
    });

    formatter = forAllSystems (pkgs: pkgs.alejandra);
  };
}
