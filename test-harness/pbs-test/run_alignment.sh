#!/bin/bash
#PBS -N run_alignment
#PBS -l nodes=1:ppn=8,mem=16G
#PBS -q workq
#PBS -o logs/run_alignment.out
#PBS -e logs/run_alignment.err

mkdir -p logs
mkdir -p aligned
echo "Alignment complete" > aligned/result.bam
echo "Threads: 8, Memory: 16G" >> aligned/result.bam
hostname >> aligned/result.bam
date >> aligned/result.bam
