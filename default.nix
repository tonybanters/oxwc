{
  lib,
  rustPlatform,
  pkg-config,
  wayland,
  libxkbcommon,
  libGL,
  libX11,
  libXcursor,
  libXrandr,
  libXi,
  gitRev ? null,
}:
rustPlatform.buildRustPackage (finalAttrs: {
  pname = "projectwc";
  version = if gitRev != null then lib.substring 0 8 gitRev else "dev";

  src = ./.;

  cargoLock = {
    lockFile = ./Cargo.lock;
    outputHashes = {
      "smithay-0.7.0" = "sha256-VTc1J3DiKUC79Jn4apUcK7XxEJmIaDXB5K0GE0OqR3g=";
    };
  };

  nativeBuildInputs = [pkg-config];

  buildInputs = [
    wayland
    libxkbcommon
    libGL
    libX11
    libXcursor
    libXrandr
    libXi
  ];

  doCheck = false;

  meta = {
    description = "Wayland compositor written in Rust using smithay";
    license = lib.licenses.gpl3Only;
    platforms = lib.platforms.linux;
    mainProgram = "projectwc";
  };
})
