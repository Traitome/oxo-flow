#!/bin/bash
#SBATCH --job-name=process_sample
#SBATCH --cpus-per-task=2
#SBATCH --mem=4G
#SBATCH --partition=workq
#SBATCH --output=logs/process_sample.out
#SBATCH --error=logs/process_sample.err

mkdir -p logs
mkdir -p outputs
for sample in sample1 sample2 sample3; do
    echo "Processing $sample on $(hostname)" > outputs/${sample}_processed.txt
    echo "Threads: 2" >> outputs/${sample}_processed.txt
    echo "Memory: 4G" >> outputs/${sample}_processed.txt
    date >> outputs/${sample}_processed.txt
done
