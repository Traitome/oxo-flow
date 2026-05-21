#!/bin/bash
#SBATCH --job-name=run_qc
#SBATCH --cpus-per-task=2
#SBATCH --mem=4G
#SBATCH --partition=workq
#SBATCH --output=logs/run_qc.out
#SBATCH --error=logs/run_qc.err

mkdir -p logs
mkdir -p qc
echo "QC Report" > qc/report.html
echo "Threads: 2, Memory: 4G" >> qc/report.html
module list >> qc/report.html 2>&1 || true
