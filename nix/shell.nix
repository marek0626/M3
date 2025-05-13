{ nixpkgs ? <nixpkgs>, system ? builtins.currentSystem }:
with import nixpkgs { inherit system; };

let
    # general dependencies
    generalInputs = [
        bashInteractive less git gawk openssh which rsync wget cpio openssl shfmt yamlfmt inferno
    ];

    # building gem5
    gem5Inputs = [
        scons gcc zlib.dev protobuf gnum4 gperftools
        python311Full python311Packages.pydot pre-commit
    ];

    # simple script that just executes the passed command as the init script for the wrapped FHS
    # environment
    fhsRunner = writeShellApplication {
        name = "m3-fhs-run";
        text = ''
            if [ $# -eq 0 ]; then
                exec bash
            else
                exec "$@"
            fi
        '';
    };

    # a script that drops the command into an FHS-compliant environment with the absolute paths
    # present that are needed by buildroot
    fhsEnv = buildFHSEnv {
        name = "m3-fhs-env";
        targetPkgs = pkgs: with pkgs; [ bashInteractive fhsRunner file ];
        runScript = "m3-fhs-run";
    };

    # building the C cross compiler
    crossInputs = [ gcc perl unzip bc flock ] ++
        lib.optional (!stdenv.isDarwin) fhsEnv ++
        lib.optional stdenv.isDarwin (runCommand "CoreFoundation" {} ''
            # I think the vanilla CoreFoundation package should add its frameworks search path
            # but it doesn’t, so we stitch together a new package here
            mkdir -p $out/nix-support
            echo NIX_LDFLAGS+=\" -F$out/Library/Frameworks\" > $out/nix-support/setup-hook
            ln -s ${darwin.apple_sdk.frameworks.CoreFoundation}/Library $out/Library
        '');

    # building the M3 system and applications
    # we want to have clang 15 for clang-format (the clang package is still at 11.1.0)
    m3Inputs = [
        rustup ninja llvmPackages_15.clang-unwrapped libxml2
        python311Packages.autopep8 python311Packages.gitpython pkg-config grcov
    ];

    # building M³Linux
    m3lxInputs = [ flex bison dtc ncurses ];

    # build system support on Darwin
    darwinInputs = lib.attrValues {
        nproc = writeScriptBin "nproc" ''#!/bin/sh
            exec sysctl -n hw.activecpu
        '';
    };

in mkShellNoCC {

    packages = generalInputs ++ gem5Inputs ++ crossInputs ++ m3Inputs ++ m3lxInputs ++
        lib.optionals stdenv.isDarwin darwinInputs;

    # "format" was required for the cross-gcc build. we now specify all, because otherwise we cannot
    # disable _FORTIFY_SOURCE anymore (which we need when building in debug mode as _FORTIFY_SOURCE
    # apparently requires at least -O1).
    hardeningDisable = [ "all" ];

    LD_LIBRARY_PATH = lib.makeLibraryPath [ stdenv.cc.cc flex ncurses python311Full ];

    shellHook = ''
        # having these set breaks some configure checks
        unset CC CXX AS LD AR RANLIB NM OBJCOPY OBJDUMP READELF SIZE STRINGS STRIP

        # if we're in the nix subdirectory (e.g., due to direnv), move one level up
        if [[ "$PWD" = */nix ]]; then
            export RUSTUP_HOME=$PWD/../.rustup
            export CARGO_HOME=$PWD/../.cargo
            export DYLINT_DRIVER_PATH=$PWD/../.dylint_drivers
        else
            export RUSTUP_HOME=$PWD/.rustup
            export CARGO_HOME=$PWD/.cargo
            export DYLINT_DRIVER_PATH=$PWD/.dylint_drivers
        fi
        export M3_TARGET=''${M3_TARGET:-gem5}
        export M3_ISA=''${M3_ISA:-riscv64}
        export M3_BUILD=''${M3_BUILD:-release}

        # determine correct terminfo directory. that is needed for ncurses, which is for example
        # used by gdb --tui. without setting TERMINFO_DIRS, a path in the cross directory will be
        # used and gdb quits as it does not find the info file there.
        for dir in /etc/terminfo /lib/terminfo /usr/share/terminfo; do
            if [ -f "$dir/''${TERM:0:1}/$TERM" ]; then
                export TERMINFO_DIRS="$dir"
                break
            fi
        done

        test -r ~/.shellrc && . ~/.shellrc
    '';
}
