{
  description = "Cellular Automata Explorer — Rust + GPUI";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs = { self, nixpkgs, rust-overlay, flake-utils }:
    flake-utils.lib.eachDefaultSystem (system:
      let
        pkgs = import nixpkgs {
          inherit system;
          overlays = [ rust-overlay.overlays.default ];
        };
        rustToolchain = pkgs.rust-bin.fromRustupToolchainFile ./rust-toolchain.toml;
      in
      {
        devShells.default = pkgs.mkShell {
          packages = with pkgs; [
            rustToolchain
            rust-analyzer
            cargo-nextest
            pkg-config
          ] ++ pkgs.lib.optionals pkgs.stdenv.isDarwin [
            # GPUI's macOS frameworks (Metal, AppKit, CoreText, …) come from
            # the stdenv's default Apple SDK; adding another SDK conflicts.
            libiconv
          ] ++ pkgs.lib.optionals pkgs.stdenv.isLinux [
            # GPUI on Linux: Vulkan + windowing/font stack
            vulkan-loader
            wayland
            libxkbcommon
            xorg.libX11
            xorg.libXcursor
            xorg.libXi
            fontconfig
            freetype
            openssl
          ];

          # No Metal toolchain needed: gpui is built with its
          # `runtime_shaders` feature, which compiles shaders at runtime via
          # the Metal framework instead of `xcrun metal` at build time.
        };
      });
}
