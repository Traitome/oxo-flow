#!/bin/bash
# Start an SoGE execution daemon inside a worker container. Each container
# carries its own image-local $SGE_ROOT, so the exec side bootstraps the cell
# skeleton (act_qmaster, bootstrap, settings.sh) itself; the qmaster contact
# info is the only cross-container fact and comes from MASTER_HOST. The
# master must already be up and this host registered as an exec host (done by
# setup-master.sh from WORKER_HOSTS).
set -euo pipefail

export SGE_ROOT=/opt/sge
export SGE_CELL=default
export SGE_ARCH=lx-amd64
MASTER=${MASTER_HOST:-sge-master}
COMMON=$SGE_ROOT/$SGE_CELL/common
SPOOL=/var/spool/gridengine/classic

test -d /work

for _ in $(seq 1 60); do
  (exec 3<>/dev/tcp/$MASTER/6444) 2>/dev/null && break
  sleep 2
done

mkdir -p "$COMMON" "$SPOOL" /var/spool/gridengine/exec/$(hostname)

echo "$MASTER" > "$COMMON/act_qmaster"

cat > "$COMMON/bootstrap" <<EOF
admin_user               root
default_domain           none
ignore_fqdn              true
spooling_method          classic
spooling_lib             libspoolc
spooling_params          $COMMON;$SPOOL
binary_path              $SGE_ROOT/bin/$SGE_ARCH
qmaster_spool_dir        /var/spool/gridengine/qmaster
security_mode            none
listener_threads         2
worker_threads           2
scheduler_threads        1
EOF

cat > "$COMMON/settings.sh" <<EOF
SGE_ROOT=$SGE_ROOT; export SGE_ROOT
SGE_CELL=$SGE_CELL; export SGE_CELL
SGE_ARCH=$SGE_ARCH; export SGE_ARCH
EOF

# sge_execd daemonizes; the container must outlive the launcher.
"$SGE_ROOT/bin/$SGE_ARCH/sge_execd"
touch /var/log/execd-alive
exec tail -f /var/log/execd-alive
