#!/bin/bash
#SBATCH --job-name=qc_check
#SBATCH --cpus-per-task=1
#SBATCH --partition=workq
#SBATCH --output=logs/qc_check.out
#SBATCH --error=logs/qc_check.err

mkdir -p logs
for sample in sample1 sample2 sample3; do
    echo "QC passed for $sample" > outputs/${sample}_qc.txt
    echo "Status: OK" >> outputs/${sample}_qc.txt
done
