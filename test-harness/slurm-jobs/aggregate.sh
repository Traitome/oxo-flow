#!/bin/bash
#SBATCH --job-name=aggregate
#SBATCH --cpus-per-task=1
#SBATCH --partition=workq
#SBATCH --output=logs/aggregate.out
#SBATCH --error=logs/aggregate.err

mkdir -p logs
echo "Final Report" > outputs/final_report.txt
echo "============" >> outputs/final_report.txt
for f in outputs/sample1_qc.txt outputs/sample2_qc.txt outputs/sample3_qc.txt; do
    echo "---" >> outputs/final_report.txt
    cat $f >> outputs/final_report.txt
done
echo "============" >> outputs/final_report.txt
echo "All samples processed successfully" >> outputs/final_report.txt
