#!/bin/bash
# Initialize an OpenPBS server + scheduler inside the master container.
set -euo pipefail

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
PBS_VERSION=23.0.6
EOF

/opt/pbs/libexec/pbs_postinstall server 2>/dev/null || /opt/pbs/libexec/pbs_postinstall
sed -i "s/^PBS_SERVER=.*/PBS_SERVER=$MASTER/" /etc/pbs.conf
echo "$MASTER" > /var/spool/pbs/server_name

/opt/pbs/sbin/pbs_server -t create 2>/dev/null || /opt/pbs/sbin/pbs_server
sleep 3

# Custom gpu resource so `nodes=1:ppn=1:gpu=1` matches without physical GPUs
qmgr -c "create resource gpu type=number, flag=nh" 2>/dev/null || true

# Register workers before their moms contact us
for h in $WORKERS; do
  qmgr -c "create node $h"
  qmgr -c "set node $h resources_available.gpu=1"
done

qmgr -c "create queue workq"
qmgr -c "set queue workq queue_type = Execution"
qmgr -c "set queue workq enabled = true"
qmgr -c "set queue workq started = true"
qmgr -c "set server default_queue = workq"
qmgr -c "set server scheduling = true"

echo "PBS server ready on $MASTER"
while true; do sleep 60; done
