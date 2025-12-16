{
  description = "oxwc - A Wayland compositor in Rust.";
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
      oxwc = default;
    });

    devShells = forAllSystems (pkgs: {
      default = pkgs.mkShell {
        inputsFrom = [self.packages.${pkgs.stdenv.hostPlatform.system}.oxwc];
        packages = [
          pkgs.rustc
          pkgs.cargo
          pkgs.clippy
          pkgs.foot
          pkgs.just
        ];
        shellHook = ''
          export PS1="(oxwc-dev) $PS1"
          export LD_LIBRARY_PATH="${pkgs.wayland}/lib:${pkgs.libxkbcommon}/lib:${pkgs.libGL}/lib:$LD_LIBRARY_PATH"
        '';
        env.RUST_SRC_PATH = "${pkgs.rustPlatform.rustLibSrc}";
      };
    });

    formatter = forAllSystems (pkgs: pkgs.alejandra);
  };
}
