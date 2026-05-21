#!/bin/bash
#PBS -N generate_report
#PBS -l nodes=1:ppn=1,mem=4G
#PBS -q workq
#PBS -o logs/generate_report.out
#PBS -e logs/generate_report.err

mkdir -p logs
mkdir -p reports
echo "<html><body><h1>Final Report</h1></body></html>" > reports/final.html
