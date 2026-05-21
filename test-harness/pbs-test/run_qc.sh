#!/bin/bash
#PBS -N run_qc
#PBS -l nodes=1:ppn=2,mem=4G
#PBS -q workq
#PBS -o logs/run_qc.out
#PBS -e logs/run_qc.err

mkdir -p logs
mkdir -p qc
echo "QC Report" > qc/report.html
echo "Threads: 2, Memory: 4G" >> qc/report.html
module list >> qc/report.html 2>&1 || true
