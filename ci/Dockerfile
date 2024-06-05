FROM ubuntu:latest

LABEL description="Build environment for M³"

RUN set -x \
    && apt-get update \
    && DEBIAN_FRONTEND=noninteractive apt-get install -y \
       apt-transport-https \
       file \
       git \
       gpg \
       htop \
       nix \
       psmisc \
       vim \
    && apt-get clean \
    && rm -rf /var/lib/apt/lists/*

# install kubectl
RUN curl -fsSL https://pkgs.k8s.io/core:/stable:/v1.29/deb/Release.key | \
        gpg --dearmor -o /etc/apt/keyrings/kubernetes-apt-keyring.gpg \
    && echo 'deb [signed-by=/etc/apt/keyrings/kubernetes-apt-keyring.gpg] ' \
        'https://pkgs.k8s.io/core:/stable:/v1.29/deb/ /' | \
        tee /etc/apt/sources.list.d/kubernetes.list \
    && apt-get update \
    && DEBIAN_FRONTEND=noninteractive apt-get install -y \
       kubectl
ARG KUBECFG
COPY $KUBECFG /root/.kube/config

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

