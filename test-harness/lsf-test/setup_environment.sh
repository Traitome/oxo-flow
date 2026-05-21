#!/bin/bash
#BSUB -J setup_environment
#BSUB -n 1
#BSUB -q workq
#BSUB -o logs/setup_environment.out
#BSUB -e logs/setup_environment.err

mkdir -p logs
mkdir -p data && echo 'sample1 sample2 sample3' > data/samples.txt