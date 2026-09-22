#!/bin/bash
# Initialize a Son of Grid Engine master inside a container (image has SoGE
# built from source under $SGE_ROOT via distinst -local -all).
#
# Classic flatfile spooling — spooling_params = "<common>;<spool>", the exact
# format spool_classic_create_context() of 8.1.9 parses. All scheduler state
# lives in the container so every `docker compose up` boots a pristine cluster.
set -euo pipefail

export SGE_ROOT=/opt/sge
export SGE_CELL=default
export SGE_ARCH=lx-amd64
COMMON=$SGE_ROOT/$SGE_CELL/common
MASTER=${MASTER_HOST:-sge-master}
WORKERS=${WORKER_HOSTS:-sge-exec1 sge-exec2}
SLOTS=${SLOTS:-2}
SPOOL=/var/spool/gridengine/classic
QMASTER_SPOOL=/var/spool/gridengine/qmaster
EXECD_SPOOL=/var/spool/gridengine/exec

test -d /work

rm -rf "$COMMON" "$SPOOL" "$QMASTER_SPOOL" "$EXECD_SPOOL"
mkdir -p "$COMMON" "$SPOOL" "$QMASTER_SPOOL" "$EXECD_SPOOL"

echo "$MASTER" > "$COMMON/act_qmaster"

cat > "$COMMON/bootstrap" <<EOF
admin_user               root
default_domain           none
ignore_fqdn              true
spooling_method          classic
spooling_lib             libspoolc
spooling_params          $COMMON;$SPOOL
binary_path              $SGE_ROOT/bin/$SGE_ARCH
qmaster_spool_dir        $QMASTER_SPOOL
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
chmod +x "$COMMON/settings.sh"

BIN=$SGE_ROOT/bin/$SGE_ARCH
UTILBIN=$SGE_ROOT/utilbin/$SGE_ARCH
LIB=$SGE_ROOT/lib/$SGE_ARCH
DIST_RES=$SGE_ROOT/util/resources
export LD_LIBRARY_PATH=$LIB
export PATH=$BIN:$PATH

$UTILBIN/spoolinit classic libspoolc "$COMMON;$SPOOL" init
$UTILBIN/spooldefaults configuration /opt/cluster/master-config
$UTILBIN/spooldefaults complexes "$DIST_RES/centry"
$UTILBIN/spooldefaults usersets "$DIST_RES/usersets"
# gpu consumable: same centry file mechanism as the shipped complexes, so
# `#$ -l gpu=<n>` resolves without site GPUs
mkdir -p /tmp/centry-gpu
cp "$DIST_RES"/centry/* /tmp/centry-gpu/
printf "name        gpu\nshortcut    G\ntype        INT\nrelop       <=\nrequestable YES\nconsumable  JOB\ndefault     0\nurgency     0\n" > /tmp/centry-gpu/gpu
$UTILBIN/spooldefaults complexes /tmp/centry-gpu
$UTILBIN/spooldefaults managers root

nohup "$BIN/sge_qmaster" >/var/log/sge-qmaster.log 2>&1 &
for _ in $(seq 1 30); do
  sleep 1
  (exec 3<>/dev/tcp/127.0.0.1/6444) 2>/dev/null && break
done
sleep 2

source "$COMMON/settings.sh"

# Consumable gpu counter must exist before hosts reference it: GPU directives
# schedule on a scheduler without physical GPUs so directive semantics remain
# observable.

# Submit + exec hosts, hostgroup, queue (the -A* verbs take a template FILE)
qconf -as "$MASTER"
# Wait for the worker containers' DNS aliases: compose starts dependents
# right after the master, so registration can outrun their first boot.
for h in $WORKERS; do
  for _ in $(seq 1 60); do
    getent hosts "$h" >/dev/null 2>&1 && break
    sleep 2
  done
done
for h in $WORKERS; do
  cat > /tmp/eh.txt <<EOF
hostname              $h
load_scaling          NONE
complex_values        h_vmem=8G,gpu=2
user_lists            NONE
xuser_lists           NONE
projects              NONE
xprojects             NONE
usage_scaling         NONE
report_variables      NONE
EOF
  qconf -Ae /tmp/eh.txt
done
printf "group_name @allhosts\nhostlist %s\n" "$WORKERS" > /tmp/hgrp.txt
qconf -Ahgrp /tmp/hgrp.txt

# Parallel environment oxo-flow's `#$ -pe smp <threads>` directive expects.
cat > /tmp/smp.pe <<EOF
pe_name            smp
slots              $SLOTS
user_lists         NONE
xuser_lists        NONE
start_proc_args    NONE
stop_proc_args     NONE
allocation_rule    \$pe_slots
control_slaves     FALSE
job_is_first_task  TRUE
urgency_slots      min
accounting_summary FALSE
EOF
qconf -Ap /tmp/smp.pe

cat > /tmp/queue.q <<EOF
qname                 all.q
hostlist              @allhosts
seq_no                0
nsuspend              1
qtype                 BATCH
slots                 $SLOTS
tmpdir                /tmp
shell                 /bin/bash
calendar              NONE
priority              0
processors            UNDEFINED
prolog                NONE
epilog                NONE
shell_start_mode      unix_behavior
starter_method        NONE
suspend_method        NONE
resume_method         NONE
terminate_method      NONE
initial_state         default
rerun                 FALSE
s_rt                  INFINITY
h_rt                  INFINITY
s_cpu                 INFINITY
h_cpu                 INFINITY
s_fsize               INFINITY
h_fsize               INFINITY
s_data                INFINITY
h_data                INFINITY
s_stack               INFINITY
h_stack               INFINITY
s_core                0
h_core                0
s_rss                 INFINITY
h_rss                 INFINITY
s_vmem                INFINITY
h_vmem                INFINITY
suspend_interval      00:05:00
notify                00:00:00
user_lists            NONE
xuser_lists           NONE
min_cpu_interval      00:05:00
ckpt_list             NONE
pe_list               smp
owner_list            NONE
projects              NONE
xprojects             NONE
load_thresholds       np_load_avg=99.0
suspend_thresholds    NONE
subordinate_list      NONE
complex_values        NONE
EOF
qconf -Aq /tmp/queue.q


echo "SGE master ready on $MASTER (queue all.q, PE smp, workers: $WORKERS)"
exec tail -f /var/log/sge-qmaster.log
