#!/bin/bash
#BSUB -J run_qc
#BSUB -n 2
#BSUB -M 4G
#BSUB -q workq
#BSUB -o logs/run_qc.out
#BSUB -e logs/run_qc.err

mkdir -p logs
mkdir -p qc
echo "QC Report" > qc/report.html
echo "Threads: 2, Memory: 4G" >> qc/report.html
module list >> qc/report.html 2>&1 || true
