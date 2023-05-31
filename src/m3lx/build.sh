#!/bin/bash

if [ $# -lt 4 ]; then
    echo "This script is not intended to be called directly. Use the commands in ./b." >&2 && exit 1
fi
if [ "$M3_ISA" != "riscv" ]; then
    echo "Only supported on M3_ISA=riscv." >&2 && exit 1
fi

crossname="$1"
crossdir="$2"
command="$3"
shift 3

root=$(readlink -f "$(dirname "$(dirname "$(dirname "$0")")")")
lxbuild="build/linux"

build_bbl() {
    bblbuild="build/riscv-pk"
    mkdir -p "$bblbuild"

    args=("--with-mem-start=0x10003000")

    (
        cd "$bblbuild" \
            && RISCV="$crossdir/.." "$root/src/m3lx/riscv-pk/configure" \
                --host=riscv64-linux \
                "--with-payload=$root/$lxbuild/vmlinux" "${args[@]}" \
            && CFLAGS=" -D__riscv_compressed=1" make "-j$(nproc)" "$@"
    )
}

case "$command" in
    mklx)
        lxdeps="$root/src/m3lx"
        # for some weird reason, the path for O needs to be relative
        makeargs=("O=../../../$lxbuild" "-j$(nproc)")
        mkdir -p "$lxbuild"

        export ARCH=riscv
        export CROSS_COMPILE="$crossname"

        # use our config, if not already present
        if [ ! -f "$lxbuild/.config" ]; then
            ( cd "$lxdeps/linux" && \
                make "${makeargs[@]}" defconfig "KBUILD_DEFCONFIG=sifive_defconfig" )
        fi

        if [ "$@" != "" ]; then
            ( cd "$lxdeps/linux" && make "${makeargs[@]}" "$@" ) || exit 1
        else
            ( cd "$lxdeps/linux" && make "${makeargs[@]}" ) || exit 1
        fi

        # bbl includes Linux
        build_bbl
        ;;

    mkbbl)
        build_bbl "$@"
        ;;
esac
