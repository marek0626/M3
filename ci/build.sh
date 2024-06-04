#!/bin/bash

set -e

flags=""
if [ "$1" = "--no-build" ] || [ "$1" = "-n" ]; then
    flags="-n"
    shift
fi

/usr/bin/env python3 ./build.py $flags prepare

cd M3

nix develop path:. -c \
    /usr/bin/env python3 ../build.py $flags build

if [ "$flags" != "" ]; then
    nix develop path:. -c \
        /benchs/gem5.sh "$@"
fi
