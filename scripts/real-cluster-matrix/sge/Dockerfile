# Son of Grid Engine 8.1.9+ built from source on ubuntu:24.04
# Classic/BDB spooling; binaries land in $SGE_ROOT via distinst -local -all.
FROM ubuntu:24.04
ENV DEBIAN_FRONTEND=noninteractive
RUN apt-get update && apt-get install -y \
      git build-essential tcsh libdb-dev libssl-dev libncurses-dev \
      libtirpc-dev default-jdk-headless procps && rm -rf /var/lib/apt/lists/*
ENV JAVA_HOME=/usr/lib/jvm/default-java
RUN git clone --depth 1 https://ghfast.top/https://github.com/gridengine/gridengine /sge-src \
 || git clone --depth 1 https://github.com/gridengine/gridengine /sge-src
# dist/util/arch only recognises kernels 2.4/2.6; let modern kernels take the
# same (fine) path so lx-amd64 is produced instead of UNSUPPORTED-<release>.
RUN sed -i 's/2\.4\.\*|2\.6\.\*)/*)/' /sge-src/source/dist/util/arch
# Bundled jemalloc includes sys/sysctl.h unconditionally, but glibc >= 2.30
# removed it; the Linux code path never calls sysctl() (it parses /proc), so
# dropping the include is safe on the Linux build.
RUN sed -i '/#include <sys\/sysctl.h>/d' /sge-src/source/3rdparty/jemalloc/jemalloc.c
# aimk compiles with -Werror; 2011-era sources warn under gcc 13, so drop it —
# warnings are tolerated for this compatibility build.
RUN sed -i 's/-Werror//g' /sge-src/source/aimk
# glibc dropped SunRPC; cull/pack.c needs XDR from libtirpc. Point the Linux
# CFLAGS at the tirpc headers and link every binary against -ltirpc (LIBS is
# the last element of each link line, so symbol order stays correct).
RUN sed -i 's|^   set CFLAGS = "$CFLAGS"$|   set CFLAGS = "$CFLAGS -I/usr/include/tirpc"|' /sge-src/source/aimk \
 && sed -i 's|set LIBS             = "-lm -lpthread"|set LIBS             = "-lm -lpthread -ltirpc"|' /sge-src/source/aimk
# spooledit is a BDB-spooling inspection tool; with -spool-classic its
# berkeleydb references have no implementation, so drop it from the build.
RUN sed -i 's/testsuidroot infotext spooldefaults spooledit spoolinit/testsuidroot infotext spooldefaults spoolinit/' /sge-src/source/utilbin/Makefile
WORKDIR /sge-src/source
# Per README.BUILD the makefiles include per-directory *_dependencies files;
# zerodepend seeds them empty so the build needs no sge_depend pass (empty
# dependencies only mean nothing is skipped on rebuild — correct, just not
# incremental).
RUN scripts/zerodepend
# Classic flatfile spooling only: the cluster initializes with libspoolc, so
# the Berkeley DB spooling backend (whose API targets db 4.4, gone from
# Ubuntu 24.04's db 5.3) is dead weight — building without it sidesteps that
# API drift entirely.
RUN ./aimk -no-dump -no-java -no-jni -no-secure -no-qmon -only-core -spool-classic > /tmp/aimk.log 2>&1; RC=$?; \
    if [ "$RC" -ne 0 ]; then tail -300 /tmp/aimk.log; exit "$RC"; fi
ENV SGE_ROOT=/opt/sge
# distinst stages util/DetectJvmLibrary.jar from the (skipped) Java build;
# an empty stand-in satisfies its file list — nothing on the headless core
# path reads it.
RUN mkdir -p /sge-src/source/dist/DetectJvmLibrary/build/jar \
        /sge-src/source/clients/gui-installer/dist \
 && touch /sge-src/source/dist/DetectJvmLibrary/build/jar/DetectJvmLibrary.jar         /sge-src/source/clients/gui-installer/dist/installer.jar
# Stage only what the headless core needs: compiled binaries plus the
# architecture-independent common tree (util/, inst scripts, resources) —
# `-all` would also try to stage the unbuilt qmon/qmake GUI artifacts.
RUN mkdir -p /opt/sge && ./scripts/distinst -local -noexit -bin -- common utiltree utilbin > /tmp/distinst.log 2>&1; RC=$?; \
    if [ "$RC" -ne 0 ]; then tail -300 /tmp/distinst.log; exit "$RC"; fi
