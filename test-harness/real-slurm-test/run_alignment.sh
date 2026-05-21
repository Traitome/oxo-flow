#!/bin/bash
#SBATCH --job-name=run_alignment
#SBATCH --cpus-per-task=8
#SBATCH --mem=16G
#SBATCH --partition=workq
#SBATCH --output=logs/run_alignment.out
#SBATCH --error=logs/run_alignment.err

mkdir -p logs
mkdir -p aligned
echo "Alignment complete" > aligned/result.bam
echo "Threads: 8, Memory: 16G" >> aligned/result.bam
hostname >> aligned/result.bam
date >> aligned/result.bam
