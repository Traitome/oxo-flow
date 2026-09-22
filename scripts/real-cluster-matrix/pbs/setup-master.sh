#!/bin/bash
# Initialize an OpenPBS server + scheduler inside the master container.
# All qmgr operations are idempotent and logged to /tmp/qmgr-setup.log so a
# recreated container can never wedge on leftover state.
set -uo pipefail

MASTER=${MASTER_HOST:-pbs-master}
WORKERS=${WORKER_HOSTS:-pbs-exec1 pbs-exec2}
export PATH=/opt/pbs/bin:/opt/pbs/sbin:$PATH
export PBS_SERVER=$MASTER

# pbs.conf for a server+sched+comm host (no mom here)
cat > /etc/pbs.conf <<EOF
PBS_HOME=/var/spool/pbs
PBS_EXEC=/opt/pbs
PBS_SERVER=$MASTER
PBS_START_SERVER=1
PBS_START_SCHED=1
PBS_START_COMM=1
PBS_START_MOM=0
PBS_ENABLE_TRQACL=N
PBS_TITLE=OpenPBS
PBS_VERSION=23.06.06
EOF

/opt/pbs/libexec/pbs_postinstall server 2>/dev/null || /opt/pbs/libexec/pbs_postinstall
sed -i "s/^PBS_SERVER=.*/PBS_SERVER=$MASTER/" /etc/pbs.conf
echo "$MASTER" > /var/spool/pbs/server_name

# First boot: /etc/init.d/pbs runs pbs_habitat to initialize the datastore
# (an embedded postgres cluster) BEFORE the server can start — do not
# pre-create PBS_HOME/datastore by hand, pg_ctl then refuses it.
/etc/init.d/pbs start || true
# initdb on first boot takes a while; wait for the server to answer.
for _ in $(seq 1 60); do
  qstat -B >/dev/null 2>&1 && break
  sleep 2
done
if ! qstat -B >/dev/null 2>&1; then
  /opt/pbs/sbin/pbs_server -t create
  sleep 8
fi

QLOG=/tmp/qmgr-setup.log
: > "$QLOG"
qm() { qmgr -c "$1" >> "$QLOG" 2>&1 || echo "FAILED: $1" >> "$QLOG"; }

# Custom gpu resource so `nodes=1:ppn=1:gpu=1` matches without physical GPUs
qm "create resource gpu type=long, flag=nh"

# Register workers before their moms contact us
for h in $WORKERS; do
  qm "create node $h"
  qm "set node $h resources_available.gpu=1"
done

qm "create queue workq"
qm "set queue workq queue_type = Execution"
qm "set queue workq enabled = true"
qm "set queue workq started = true"
qm "set server default_queue = workq"
qm "set server scheduling = true"
# Root submits the matrix jobs: PBS denies root by default (acl_roots) and
# needs the exec-side uid mapping flattened (containers all run as root).
qm "set server acl_roots += root@*"
qm "set server flatten_files = true"
# Keep finished jobs answerable by `qstat -x` — the driver settles from it.
qm "set server job_history_enable = true"

grep -c FAILED "$QLOG" || true
echo "PBS server ready on $MASTER"
while true; do sleep 60; done
