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
    initrd="$build/rootfs.cpio"
    mkdir -p "$bblbuild"

    # determine initrd size
    initrd_size=$(stat --printf="%s" "$initrd")
    # round up to page size
    initrd_size=$(python -c "print('{}'.format(($initrd_size + 0xFFF) & 0xFFFFF000))")
    # we always place the initrd at the end of the memory region (512M currently)
    initrd_end=$(printf "%#x" $((0x10000000 + 512 * 1024 * 1024)))
    initrd_start=$(printf "%#x" $((initrd_end - initrd_size)))
    sed -e "s/linux,initrd-start = <.*>;/linux,initrd-start = <$initrd_start>;/g" \
        -e "s/linux,initrd-end = <.*>;/linux,initrd-end = <$initrd_end>;/g" \
        "$root/m3lx/configs/$M3_TARGET.dts" > "$bblbuild/$M3_TARGET.dts" || exit 1

    args=("--with-mem-start=0x10003000" "--with-dts=$M3_TARGET.dts")

    (
        cd "$bblbuild" \
            && RISCV="$crossdir/.." "$root/m3lx/riscv-pk/configure" \
                --host=riscv64-linux \
                "--with-payload=$root/$lxbuild/vmlinux" "${args[@]}" \
            && CFLAGS=" -D__riscv_compressed=1" make "-j$(nproc)" "$@"
    )
    cp "$bblbuild/bbl" "$build/bbl"
}

case "$command" in
    mklx)
        lxdeps="$root/m3lx"
        # for some weird reason, the path for O needs to be relative
        makeargs=("O=../../$lxbuild" "-j$(nproc)")
        mkdir -p "$lxbuild"

        export ARCH=riscv
        export CROSS_COMPILE="$crossname"

        # use our config, if not already present
        if [ ! -f "$lxbuild/.config" ]; then
            ( cd "$lxdeps/linux" && \
                make "${makeargs[@]}" defconfig "KBUILD_DEFCONFIG=sifive_defconfig" )
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
        for f in "$build"/lxbin/*; do
            "${crossprefix}strip" -o "build/lxrootfs/$(basename "$f")" "$f"
        done
        cp -a m3lx/rootfs/* build/lxrootfs

        # rebuild rootfs image
        ( cd cross && ./build.sh "$M3_ISA" "$@" )

        cp "$crossdir/../../images/rootfs.cpio" "$build/rootfs.cpio"

        # now rebuild the dts to include the correct initrd size
        build_bbl
        ;;
esac
