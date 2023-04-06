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

crossprefix="$crossdir/$crossname"
root=$(readlink -f "$(dirname "$(dirname "$0")")")
build="build/$M3_TARGET-$M3_ISA-$M3_BUILD"
lxbuild="build/linux"

build_bbl() {
    bblbuild="build/riscv-pk/$M3_TARGET"
    initrd="build/cross-riscv/images/rootfs.cpio"
    mkdir -p "$bblbuild"

    if [ "$M3_TARGET" = "gem5" ]; then
        args=("--with-mem-start=0x10200000")
    else
        # determine initrd size
        initrd_start=0x14000000
        initrd_size=$(stat --printf="%s" "$initrd")
        # replace the end in the hw.dts
        initrd_end=$(printf "%#x" $((initrd_start + initrd_size)))
        sed -e \
            "s/linux,initrd-end = <0x14400000>;/linux,initrd-end = <$initrd_end>;/g" \
            "$root/m3lx/configs/hw.dts" > "$bblbuild/hw.dts"

        args=("--with-mem-start=0x10001000" "--with-dts=hw.dts")
    fi

    (
        cd "$bblbuild" \
            && RISCV="$crossdir/.." "$root/m3lx/riscv-pk/configure" \
                --host=riscv64-linux \
                "--with-payload=$root/$lxbuild/vmlinux" "${args[@]}" \
            && CFLAGS=" -D__riscv_compressed=1" make "-j$(nproc)" "$@"
    )
}

case "$command" in
    mklx)
        lxdeps="$root/m3lx"
        # for some weird reason, the path for O needs to be relative
        makeargs=("O=../../$lxbuild" "-j$(nproc)")
        mkdir -p "$lxbuild"

        if [ ! -f "$lxbuild/.config" ]; then
            cp "$lxdeps/configs/config-linux-riscv64" "$lxbuild/.config"
        fi

        export ARCH=riscv
        export CROSS_COMPILE="$crossname"

        # use our config, if not already present
        if [ ! -f "$lxbuild/.config" ]; then
            ( cd "$lxdeps/linux" && \
                make "${makeargs[@]}" defconfig "KBUILD_DEFCONFIG=$lxdeps/config-linux-$ARCH" )
        fi

        ( cd "$lxdeps/linux" && make "${makeargs[@]}" "$@" ) || exit 1

        # bbl includes Linux
        build_bbl
        ;;

    mkbbl)
        build_bbl "$@"
        ;;

    mkrootfs)
        # copy binaries to overlay directory for buildroot and strip them
        mkdir -p "build/lxrootfs"
        for f in "$build"/lxbins/*; do
            "${crossprefix}strip" -o "build/lxrootfs/$(basename "$f")" "$f"
        done
        cp -a m3lx/rootfs/* build/lxrootfs

        # rebuild rootfs image
        ( cd cross && ./build.sh "$M3_ISA" "$@" )

        # now rebuild the dts to include the correct initrd size
        build_bbl
        ;;
esac
