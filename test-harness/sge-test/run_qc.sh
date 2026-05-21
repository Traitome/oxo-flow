#!/bin/bash
#$ -N run_qc
#$ -pe smp 2
#$ -l h_vmem=4G
#$ -q workq
#$ -o logs/run_qc.out
#$ -e logs/run_qc.err

mkdir -p logs
mkdir -p qc
echo "QC Report" > qc/report.html
echo "Threads: 2, Memory: 4G" >> qc/report.html
module list >> qc/report.html 2>&1 || true
