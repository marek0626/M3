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

lx_deps_root="$(dirname $(readlink -f "$0"))"
build="$lx_deps_root/../build/$M3_TARGET-$M3_ISA-$M3_BUILD/linux-deps"
m3_root="$lx_deps_root/.."


buildroot_dir="$build/buildroot"
disks_dir="$build/disks"
linux_dir="$build/linux"
bbl_dir="$build/bbl"

skip_linux_build=false
cpu_type="TimingSimpleCPU"
debug_start_on_boot=false

mkdir -p "$buildroot_dir" "$disks_dir" "$linux_dir" "$bbl_dir"

main() {
	for arg in "$@"; do
		case $arg in
			--skip-lx-build)
				skip_linux_build=true
				;;
			--o3cpu)
				cpu_type="DerivO3CPU"
				;;
			--debug-start-on-boot)
				debug_start_on_boot=true
				;;
			*)
				echo "unknown option: $arg" >&2
				exit 1
				;;
		esac
	done


	# we need the cross compiler from buildroot to build the userspace programs
	if [ ! -f "$buildroot_dir/host/bin/riscv64-linux-gcc" ]; then
		mk_buildroot
	fi

	# TODO: this is awkward – actually, $m3_root/b should call this script and not the other way around
	"$m3_root/b" || exit 1

	# copy the lxclient executable to the buildroot file system (rootfs)
	# TODO: don't hardcode this (the next 6 lines)
	rustbin="$m3_root/build/rust/riscv64gc-unknown-linux-gnu/release"
	apps="lxclient simplebench"
	for app in $apps; do
		full_app="$rustbin/$app"
		if [ ! -f "$full_app" ]; then
			echo "linux program $full_app does not exist" >&2
			exit 1
		fi
		cp "$full_app" "$buildroot_dir/target/"
	done

	mk_buildroot

	if [ "$skip_linux_build" = false ]; then
		mk_linux
		mk_bbl
	fi

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
                --with-mem-start=0x30000000 \
            && CFLAGS=" -D__riscv_compressed=1" make -j$(nproc)
    )
    if [ $? -ne 0 ]; then
        echo "bbl/riscv-pk compilation failed" >&2
        exit 1
    fi
}

run_gem5() {
    # TODO check validity, extract config/dom/app etc.
    # TODO support different boot scripts
    cp boot/linux/m3fs.xml run/boot.xml
    export M3_GEM5_FS="$lx_deps_root/../build/$M3_TARGET-$M3_ISA-$M3_BUILD/default.img"

    "$m3_root/platform/gem5/build/RISCV/gem5.opt" \
        "--outdir=$m3_root/run" \
        `if [ -n "$debug_flags" ]; then echo "--debug-flags=$debug_flags"; fi` \
        --debug-file=gem5.log \
        `if [ "$debug_start_on_boot" = true ]; then echo "--debug-start=506053425500"; fi` \
        "$m3_root/config/linux.py" \
        --disk-image "$disks_dir/root.img" \
        --kernel "$bbl_dir/bbl" \
        --mods $m3_root/run/boot.xml,$m3_root/build/gem5-riscv-release/bin/root,$m3_root/build/gem5-riscv-release/bin/pager,$m3_root/build/gem5-riscv-release/bin/m3fs \
        --cpu-type "$cpu_type"
}

main "$@"
