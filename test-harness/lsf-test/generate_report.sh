#!/bin/bash
#BSUB -J generate_report
#BSUB -n 1
#BSUB -M 4G
#BSUB -q workq
#BSUB -o logs/generate_report.out
#BSUB -e logs/generate_report.err

mkdir -p logs
mkdir -p reports
echo "<html><body><h1>Final Report</h1></body></html>" > reports/final.html
