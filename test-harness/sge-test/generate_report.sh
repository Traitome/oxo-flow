#!/bin/bash
#$ -N generate_report
#$ -pe smp 1
#$ -l h_vmem=4G
#$ -q workq
#$ -o logs/generate_report.out
#$ -e logs/generate_report.err

mkdir -p logs
mkdir -p reports
echo "<html><body><h1>Final Report</h1></body></html>" > reports/final.html
