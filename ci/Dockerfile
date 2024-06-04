FROM ubuntu:latest

LABEL description="Build environment for M³"

RUN set -x \
    && apt-get update \
    && DEBIAN_FRONTEND=noninteractive apt-get install -y \
       file \
       git \
       htop \
       nix \
       psmisc \
       vim \
    && apt-get clean \
    && rm -rf /var/lib/apt/lists/*

RUN mkdir -p $HOME/.ssh \
    && ssh-keyscan -H github.com >> $HOME/.ssh/known_hosts \
    && ssh-keyscan -H gitlab.barkhauseninstitut.org >> $HOME/.ssh/known_hosts

RUN mkdir -p $HOME/.config/nix && \
    echo "experimental-features = nix-command flakes" \
        > $HOME/.config/nix/nix.conf

ARG CACHE
COPY $CACHE /cache

COPY benchs/ /benchs/
COPY input/ /input/
COPY build.sh /
COPY build.py /

