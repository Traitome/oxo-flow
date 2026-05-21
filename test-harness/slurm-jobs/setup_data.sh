#!/bin/bash
#SBATCH --job-name=setup_data
#SBATCH --cpus-per-task=1
#SBATCH --partition=workq
#SBATCH --output=logs/setup_data.out
#SBATCH --error=logs/setup_data.err

mkdir -p logs
mkdir -p test-data
# Create sample list
echo "sample1" > test-data/samples.txt
echo "sample2" >> test-data/samples.txt
echo "sample3" >> test-data/samples.txt
# Create reference file
echo ">ref" > test-data/reference.fa
echo "ATCGATCGATCGATCGATCGATCG" >> test-data/reference.fa
