#!/bin/bash

set -e

args=$(getopt -o c:d:i --long commit:,cache-dir:,incremental -n "$0" -- "$@")
eval set -- "$args"

commit="origin/ci"
cachedir="ci/out"
incremental=""
while true; do
    case "$1" in
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
    cd M3 && git fetch origin
fi
git checkout "$commit"
if [ "$incremental" = "-i" ]; then
    git submodule sync
fi

/usr/bin/env python3 \
    ./ci/builder.py --cache-dir "$cachedir" $incremental prepare
nix develop path:. -c \
    /usr/bin/env python3 \
        ./ci/builder.py --cache-dir "$cachedir" $incremental build
