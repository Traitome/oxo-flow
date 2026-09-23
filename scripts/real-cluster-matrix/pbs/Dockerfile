# OpenPBS 23.06.06 built from source on ubuntu:24.04 (no official debs are
# attached to the GitHub releases; this yields the same real PBS daemons).
FROM ubuntu:24.04
ENV DEBIAN_FRONTEND=noninteractive
RUN apt-get update && apt-get install -y git \
      build-essential autoconf automake libtool pkg-config \
      libssl-dev libhwloc-dev libedit-dev libncurses-dev libexpat1-dev \
      libx11-dev libxt-dev libdb-dev libpq-dev libical-dev libxml2-dev tcl-dev tk-dev swig \
      python3 python3-dev gettext procps curl postgresql postgresql-client \
      && rm -rf /var/lib/apt/lists/*
RUN git clone --depth 1 --branch v23.06.06 https://ghfast.top/https://github.com/openpbs/openpbs /pbs-src \
 || git clone --depth 1 --branch v23.06.06 https://github.com/openpbs/openpbs /pbs-src
WORKDIR /pbs-src
# Python 3.12 removed Py_SetProgramName; PBS 23.06's only call site predates
# that. Guard it so the embedded interpreter init compiles on noble.
RUN sed -i 's|^\tPy_SetProgramName(w_python_binpath);|\t#if PY_VERSION_HEX < 0x030B0000\n\tPy_SetProgramName(w_python_binpath);\n\t#endif|' \
      /pbs-src/src/lib/Libpython/shared_python_utils.c
RUN sed -i '/#include <eval.h>/d' /pbs-src/src/lib/Libpython/pbs_python_external.c
RUN ./autogen.sh > /tmp/pbs-autogen.log 2>&1; RC=$?; \
    if [ "$RC" -ne 0 ]; then tail -40 /tmp/pbs-autogen.log; exit "$RC"; fi
RUN ./configure --prefix=/opt/pbs > /tmp/pbs-configure.log 2>&1; RC=$?; \
    if [ "$RC" -ne 0 ]; then tail -40 /tmp/pbs-configure.log; exit "$RC"; fi
RUN make -j3 > /tmp/pbs-make.log 2>&1; RC=$?; \
    if [ "$RC" -ne 0 ]; then grep -iE "error:" /tmp/pbs-make.log | head -12; exit "$RC"; fi
RUN make install > /tmp/pbs-install.log 2>&1; RC=$?; \
    if [ "$RC" -ne 0 ]; then tail -40 /tmp/pbs-install.log; exit "$RC"; fi
# pbs_postinstall/pbs_dataserver need the postgres tooling on PATH
RUN ln -sf /usr/lib/postgresql/*/bin/* /usr/local/bin/
ENV PATH=/opt/pbs/bin:/opt/pbs/sbin:$PATH
