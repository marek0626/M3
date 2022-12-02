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

buildroot_dir="$build/buildroot"
linux_dir="$build/linux"

if [ ! -d "$buildroot_dir/host/usr/bin" ]; then
	echo "buildroot does not exist"
	exit 1
fi

mkdir -p "$linux_dir"

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
