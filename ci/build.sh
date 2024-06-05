#!/bin/bash

set -ex

REPO='https://ci:gldt-Y6KXy-AKNDZ8uUb8PnY3@gitlab.barkhauseninstitut.org/os/code/M3/M3.git'

args=$(getopt -o c:n: --long commit:,no-build -n "$0" -- "$@")
eval set -- "$args"

commit="origin/ci"
bargs=""
while true; do
    case "$1" in
        -n | --no-build)
            bargs="-n"
            shift 1
            ;;
        -c | --commit)
            commit="$2"
            shift 2
            ;;
        --)
            shift
            break
            ;;
        *)
            break
            ;;
    esac
done

[ -d M3 ] || git clone "$REPO"
cd M3
git checkout "$commit"

/usr/bin/env python3 ./ci/cache.py $bargs prepare
nix develop path:. -c \
    /usr/bin/env python3 ./ci/cache.py $bargs build
