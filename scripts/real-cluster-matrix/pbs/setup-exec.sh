#!/bin/bash
# Start an OpenPBS MOM inside a worker container.
set -euo pipefail

MASTER=${MASTER_HOST:-pbs-master}
export PATH=/opt/pbs/bin:/opt/pbs/sbin:$PATH

for _ in $(seq 1 60); do
  (exec 3<>/dev/tcp/$MASTER/15001) 2>/dev/null && break
  sleep 2
done

cat > /etc/pbs.conf <<EOF
PBS_HOME=/var/spool/pbs
PBS_EXEC=/opt/pbs
PBS_SERVER=$MASTER
PBS_START_SERVER=0
PBS_START_SCHED=0
PBS_START_COMM=0
PBS_START_MOM=1
PBS_ENABLE_TRQACL=N
PBS_TITLE=OpenPBS
PBS_VERSION=23.0.6
EOF

/opt/pbs/libexec/pbs_postinstall mom 2>/dev/null || /opt/pbs/libexec/pbs_postinstall
mkdir -p /var/spool/pbs
echo "$MASTER" > /var/spool/pbs/server_name
printf '$clienthost %s\n' "$MASTER" > /var/spool/pbs/mom_priv/config

exec /opt/pbs/sbin/pbs_mom -p
