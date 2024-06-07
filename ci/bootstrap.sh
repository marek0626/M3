#!/bin/bash

set -e

args=$(getopt -o hc:d:i --long help,commit:,cache-dir:,incremental -n "$0" -- "$@")
eval set -- "$args"

usage() {
    echo "Usage: $0 [-c|--commit <commit>] [-d|--cache-dir <dir>] [-i|--incremental]"
    echo
    echo "  --commit     : the M³ commit to checkout (origin/dev by default)"
    echo "  --cache-dir  : the cache directory to use during build"
    echo "  --incremental: if set, the cache is not used, but everything is"
    echo "                 built in place and incrementally."
    exit 1
}

commit="origin/dev"
cachedir="ci/out"
incremental=""
nixargs="path:."
while true; do
    case "$1" in
        -h | --help)
            usage
            ;;
        -c | --commit)
            commit="$2"
            shift 2
            ;;
        -d | --cache-dir)
            cachedir="$2"
            shift 2
            ;;
        -i | --incremental)
            incremental="-i"
            # if building incrementally, we have a huge build directory etc. and thus we don't want
            # to put all of that into the nix store.
            nixargs=""
            shift 1
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

if [ ! -d M3 ]; then
    user=$(cat "$HOME/.gitlab/user")
    pw=$(cat "$HOME/.gitlab/pw")
    repo="https://$user:$pw@gitlab.barkhauseninstitut.org/os/code/M3/M3.git"
    git clone "$repo" && cd M3
else
    cd M3
    if [ "$incremental" = "-i" ]; then
        git fetch origin
    fi
fi
git checkout "$commit"
if [ "$incremental" = "-i" ]; then
    git submodule sync
fi

/usr/bin/env python3 \
    ./ci/builder.py --cache-dir "$cachedir" $incremental prepare
nix develop $nixargs -c \
    /usr/bin/env python3 \
        ./ci/builder.py --cache-dir "$cachedir" $incremental build
