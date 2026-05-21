#!/bin/bash
#BSUB -J run_alignment
#BSUB -n 8
#BSUB -M 16G
#BSUB -q workq
#BSUB -o logs/run_alignment.out
#BSUB -e logs/run_alignment.err

mkdir -p logs
mkdir -p aligned
echo "Alignment complete" > aligned/result.bam
echo "Threads: 8, Memory: 16G" >> aligned/result.bam
hostname >> aligned/result.bam
date >> aligned/result.bam
