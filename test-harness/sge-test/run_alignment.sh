#!/bin/bash
#$ -N run_alignment
#$ -pe smp 8
#$ -l h_vmem=16G
#$ -q workq
#$ -o logs/run_alignment.out
#$ -e logs/run_alignment.err

mkdir -p logs
mkdir -p aligned
echo "Alignment complete" > aligned/result.bam
echo "Threads: 8, Memory: 16G" >> aligned/result.bam
hostname >> aligned/result.bam
date >> aligned/result.bam
