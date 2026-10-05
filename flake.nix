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
      devShells.${system}.default = pkgs.mkShell {
        packages = [
          pkgs.cargo
          pkgs.rustc
        ];
      };
    };
}
