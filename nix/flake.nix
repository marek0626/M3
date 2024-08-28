{
    inputs = {
      nixpkgs-current.url = "github:NixOS/nixpkgs/nixos-24.05";
    };

    outputs = { self, nixpkgs, nixpkgs-current }: let

        # flake support
        lib = import "${nixpkgs}/lib";
        forAll = list: f: lib.genAttrs list f;

        current = import nixpkgs-current {
          system = "x86_64-linux";
        };

    in {
        devShells = forAll [ "x86_64-linux" "x86_64-darwin" ] (system: {
            default = import ./shell.nix { inherit nixpkgs current system; };
        });
    };
}
