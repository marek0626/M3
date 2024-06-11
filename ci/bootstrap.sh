#!/usr/bin/env bash

set -e

commit="$1"

# clone the repo with the preinstalled user&pw (but not twice)
if [ ! -d M3 ]; then
    user=$(cat "$HOME/.gitlab/user")
    pw=$(cat "$HOME/.gitlab/pw")
    repo="https://$user:$pw@gitlab.barkhauseninstitut.org/os/code/M3/M3.git"
    git clone "$repo"
fi
cd M3
git checkout "$commit"

/usr/bin/env python3 \
    ./ci/builder.py prepare
nix develop path:. -c \
    /usr/bin/env python3 \
        ./ci/builder.py build --build debug bench
