# OpenPBS 23.06.06 built from source on ubuntu:24.04 (no official debs are
# attached to the GitHub releases; this yields the same real PBS daemons).
FROM ubuntu:24.04
ENV DEBIAN_FRONTEND=noninteractive
RUN apt-get update && apt-get install -y git \
      build-essential autoconf automake libtool pkg-config \
      libssl-dev libhwloc-dev libedit-dev libncurses-dev libexpat1-dev \
      python3 python3-dev gettext procps curl \
      && rm -rf /var/lib/apt/lists/*
RUN git clone --depth 1 --branch v23.06.06 https://github.com/openpbs/openpbs /pbs-src
WORKDIR /pbs-src
RUN ./autogen.sh > /tmp/pbs-autogen.log 2>&1 \
 && ./configure --prefix=/opt/pbs > /tmp/pbs-configure.log 2>&1 \
 && make -j3 > /tmp/pbs-make.log 2>&1 \
 && make install > /tmp/pbs-install.log 2>&1
ENV PATH=/opt/pbs/bin:/opt/pbs/sbin:$PATH
