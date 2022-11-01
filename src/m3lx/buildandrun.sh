#!/bin/bash

if [ "$M3_TARGET" != 'gem5' ]; then
    echo '$M3_TARGET other than gem5 is not supported' >&2
    exit 1
fi

if [ "$M3_ISA" != 'riscv' ]; then
    echo '$M3_ISA other than riscv is not supported' >&2
    exit 1
fi

M3_BUILD="${M3_BUILD:-release}"

lx_deps_root="$(dirname "$0")"
build="$lx_deps_root/../build/$M3_TARGET-$M3_ISA-$M3_BUILD/linux-deps"
m3_root="$lx_deps_root/.."


buildroot_dir="$build/buildroot"
disks_dir="$build/disks"
linux_dir="$build/linux"
bbl_dir="$build/bbl"

mkdir -p "$buildroot_dir" "$disks_dir" "$linux_dir" "$bbl_dir"

main() {
	# TODO: this is awkward – actually, $m3_root/b should call this script and not the other way around
	PATH="$m3_root/build/gem5-riscv-release/linux-deps/buildroot/host/bin:$PATH"  "$m3_root/b" || exit 1

	# copy the lxclient executable to the buildroot file system (rootfs)
	# TODO: don't hardcode this (the next 6 lines)
	lxclient="$m3_root/build/rust/riscv64gc-unknown-linux-gnu/release/lxclient"
	if [ ! -f "$lxclient" ]; then
		echo "lxclient does not exist" >&2
		exit 1
	fi
	cp "$lxclient" "$buildroot_dir/target/"

	mk_buildroot
	mk_linux
	mk_bbl

	run_gem5
}

mk_buildroot() {
    if [ ! -f $buildroot_dir/.config ]; then
        cp "$lx_deps_root/configs/config-buildroot-riscv64" "$buildroot_dir/.config"
    fi

    ( cd "$lx_deps_root/buildroot" && make "O=$buildroot_dir" -j$(nproc) )
    if [ $? -ne 0 ]; then
        echo "buildroot compilation failed" >&2
        exit 1
    fi

    rm -f "$disks_dir/root.img"
    "$m3_root/platform/gem5/util/gem5img.py" init "$disks_dir/root.img" 128
    tmp=`mktemp -d`
    "$m3_root/platform/gem5/util/gem5img.py" mount "$disks_dir/root.img" $tmp
    cpioimg=`readlink -f $buildroot_dir/images/rootfs.cpio`
    ( cd $tmp && sudo cpio -id < $cpioimg )
    "$m3_root/platform/gem5/util/gem5img.py" umount $tmp
    rmdir $tmp
}

mk_linux() {
    if [ ! -f "$linux_dir/.config" ]; then
        cp "$lx_deps_root/configs/config-linux-riscv64" "$linux_dir/.config"
    fi

    ( 
        export PATH="$buildroot_dir/host/usr/bin:$PATH"
        export ARCH=riscv
        export CROSS_COMPILE=riscv64-linux-
        cd "$lx_deps_root/linux" && make "O=$linux_dir" -j$(nproc)
    )
    if [ $? -ne 0 ]; then
        echo "linux compilation failed" >&2
        exit 1
    fi
}

mk_bbl() {
    (
        export PATH="$buildroot_dir/host/usr/bin:$PATH"
        cd "$bbl_dir" \
            && RISCV="$buildroot_dir/host" "$lx_deps_root/riscv-pk/configure" \
                --host=riscv64-linux \
                "--with-payload=$linux_dir/vmlinux" \
                --with-mem-start=0x80000000 \
            && CFLAGS=" -D__riscv_compressed=1" make -j$(nproc)
    )
    if [ $? -ne 0 ]; then
        echo "bbl/riscv-pk compilation failed" >&2
        exit 1
    fi
}

run_gem5() {
    "$m3_root/platform/gem5/build/RISCV/gem5.opt" \
        "--outdir=$m3_root/run" \
        `if [ -n "$debug_flags" ]; then echo "--debug-flags=$debug_flags"; fi` \
        --debug-file=gem5.log \
        "$m3_root/config/linux.py" \
        --disk-image "$disks_dir/root.img" \
        --kernel "$bbl_dir/bbl" \
        --cpu-type TimingSimpleCPU
}

main
