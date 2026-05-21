#!/bin/bash
#PBS -N setup_environment
#PBS -l nodes=1:ppn=1
#PBS -q workq
#PBS -o logs/setup_environment.out
#PBS -e logs/setup_environment.err

mkdir -p logs
mkdir -p data && echo 'sample1 sample2 sample3' > data/samples.txt