{
  description = "goatty: development shell (Rust toolchain, just, GPUI system libraries)";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";
    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs = { nixpkgs, flake-utils, ... }:
    flake-utils.lib.eachSystem [ "x86_64-linux" "aarch64-linux" ] (system:
      let
        pkgs = nixpkgs.legacyPackages.${system};

        # System libraries GPUI needs; keep in sync with scripts/ci-deps.sh.
        buildLibs = with pkgs; [
          fontconfig
          freetype
          wayland
          libxkbcommon
          libx11
          libxcb
          vulkan-loader
          alsa-lib
          zstd
          openssl
        ];
      in {
        devShells.default = pkgs.mkShell {
          packages = with pkgs; [
            # Rust: edition 2024, CI runs stable with rustfmt + clippy.
            rustc
            cargo
            clippy
            rustfmt
            rust-analyzer

            just

            # Native build tooling (see scripts/ci-deps.sh).
            pkg-config
            cmake
            clang

            # goatty-shell's managed-zsh tests skip without zsh.
            zsh
          ] ++ buildLibs;

          # rust-analyzer needs the std sources.
          RUST_SRC_PATH = "${pkgs.rustPlatform.rustLibSrc}";

          # GPUI dlopens Wayland/X11/Vulkan/xkbcommon at runtime.
          LD_LIBRARY_PATH = pkgs.lib.makeLibraryPath buildLibs;
        };

        formatter = pkgs.nixfmt-rfc-style;
      });
}
