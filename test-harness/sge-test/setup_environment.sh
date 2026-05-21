#!/bin/bash
#$ -N setup_environment
#$ -pe smp 1
#$ -q workq
#$ -o logs/setup_environment.out
#$ -e logs/setup_environment.err

mkdir -p logs
mkdir -p data && echo 'sample1 sample2 sample3' > data/samples.txt