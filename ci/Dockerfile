FROM ubuntu:latest

LABEL description="Build environment for M³"

# install basic packages
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
       rsync \
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

# user and password for the BI gitlab (pull token)
ARG GITLAB_USER
COPY $GITLAB_USER /root/.gitlab/user
ARG GITLAB_PW
COPY $GITLAB_PW /root/.gitlab/pw

# use submodules from gitlab, not github (except linux-bench)
RUN ( \
        echo '[url "https://github.com/Barkhausen-Institut/linux-bench.git"]' && \
        echo '	insteadOf = https://github.com/Barkhausen-Institut/linux-bench.git' && \
        echo -n '[url "https://'$(cat /root/.gitlab/user)':'$(cat /root/.gitlab/pw) && \
        echo '@gitlab.barkhauseninstitut.org/os/code/M3/"]' && \
        echo '	insteadOf = https://github.com/Barkhausen-Institut/' \
    ) > $HOME/.gitconfig

# resolve the hosts that we need during git clone to prevent human interaction
RUN mkdir -p $HOME/.ssh \
    && ssh-keyscan -H github.com >> $HOME/.ssh/known_hosts \
    && ssh-keyscan -H gitlab.barkhauseninstitut.org >> $HOME/.ssh/known_hosts

# allow the usage of the new nix-command syntax and flakes
RUN mkdir -p $HOME/.config/nix && \
    echo "experimental-features = nix-command flakes" \
        > $HOME/.config/nix/nix.conf

COPY bootstrap.sh /

