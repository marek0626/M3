#!/bin/bash

set -ex

create_cache() {
    mkdir -p cache
}

create_build_con() {
    podman build --build-arg "CACHE=empty" -t m3-build .
}

create_test_con() {
    podman build --build-arg "CACHE=empty" -t m3-test .
}

exec_build_con() {
    podman run -t -v "$(readlink -f cache):/cache" m3-build ./build.sh
    podman image rm -f m3-build:latest
}

exec_test_con() {
    podman run -t -v "$(readlink -f cache):/cache" m3-test \
        ./build.sh -n --isa riscv64 --type a /results
    podman image rm -f m3-test:latest
}

exec_shell() {
    podman run -ti -v "$(readlink -f cache):/cache" "$1" bash
}
