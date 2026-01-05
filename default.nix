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
  systemd,
  libinput,
  seatd,
  libdrm,
  mesa,
  libgbm,
  gitRev ? null,
}:
rustPlatform.buildRustPackage (finalAttrs: {
  pname = "projectwc";
  version =
    if gitRev != null
    then lib.substring 0 8 gitRev
    else "dev";

  src = ./.;

  cargoLock = {
    lockFile = ./Cargo.lock;
    outputHashes = {
      "smithay-0.7.0" = "sha256-nLG1xqbZQ26BRTIHLPr5kK+I2J78ir4P3fUnT9vb4ek=";
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
    systemd
    libinput
    seatd
    libdrm
    mesa
    libgbm
  ];

  doCheck = false;

  meta = {
    description = "Wayland compositor written in Rust using smithay";
    license = lib.licenses.gpl3Only;
    platforms = lib.platforms.linux;
    mainProgram = "projectwc";
  };
})
