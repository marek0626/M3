FROM ubuntu:latest

LABEL description="WebUI for the M³-CI results"

# install basic packages
RUN set -x \
    && apt-get update \
    && DEBIAN_FRONTEND=noninteractive apt-get install -y \
       htop \
       nginx \
       psmisc \
       vim \
    && apt-get clean \
    && rm -rf /var/lib/apt/lists/*

# nginx
COPY nginx.cfg /etc/nginx/sites-available/default
EXPOSE 80/tcp

CMD ["/usr/sbin/nginx", "-g", "daemon off;"]
