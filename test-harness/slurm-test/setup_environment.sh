#!/bin/bash
#SBATCH --job-name=setup_environment
#SBATCH --cpus-per-task=1
#SBATCH --partition=workq
#SBATCH --output=logs/setup_environment.out
#SBATCH --error=logs/setup_environment.err

mkdir -p logs
mkdir -p data && echo 'sample1 sample2 sample3' > data/samples.txt