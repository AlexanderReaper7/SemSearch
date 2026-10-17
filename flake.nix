{
  description = "Local semantic search over code, files and web history";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

  outputs =
    { self, nixpkgs }:
    let
      system = "x86_64-linux";
      pkgs = nixpkgs.legacyPackages.${system};
    in
    {
      # nixcfg calls nix/package.nix itself to pass its CPU arch, and reads
      # config/hister.yml from this flake's source.
      packages.${system}.default = pkgs.callPackage ./nix/package.nix { };
      # The desktop window in app/ builds against GTK 4 and runs Chromium
      # through CEF, whose prebuilt libcef.so finds its libraries through
      # LD_LIBRARY_PATH. Its UI in ui/ builds with Node.
      devShells.${system}.default =
        let
          cefLibs = with pkgs; [
            alsa-lib at-spi2-atk atk cairo cups dbus expat glib gtk3 libdrm
            libgbm libGL libxkbcommon mesa nspr nss pango udev vulkan-loader
            libx11 libxcomposite libxdamage libxext
            libxfixes libxrandr libxcb libxshmfence
          ];
        in
        pkgs.mkShell {
          packages = [
            pkgs.cargo
            pkgs.rustc
            pkgs.nodejs
            pkgs.pkg-config
            pkgs.cmake
            pkgs.ninja
          ];
          buildInputs = [
            pkgs.gtk4
            pkgs.openssl
          ] ++ cefLibs;
          LD_LIBRARY_PATH = pkgs.lib.makeLibraryPath cefLibs;
        };
    };
}
