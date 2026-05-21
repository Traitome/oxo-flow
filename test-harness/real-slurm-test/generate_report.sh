#!/bin/bash
#SBATCH --job-name=generate_report
#SBATCH --cpus-per-task=1
#SBATCH --mem=4G
#SBATCH --partition=workq
#SBATCH --output=logs/generate_report.out
#SBATCH --error=logs/generate_report.err

mkdir -p logs
mkdir -p reports
echo "<html><body><h1>Final Report</h1></body></html>" > reports/final.html
