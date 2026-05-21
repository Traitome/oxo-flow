#!/bin/bash
#$ -N gpu_variant_calling
#$ -pe smp 4
#$ -l h_vmem=32G
#$ -l gpu=1
#$ -q workq
#$ -o logs/gpu_variant_calling.out
#$ -e logs/gpu_variant_calling.err

mkdir -p logs
module load cuda/12.0 gatk/4.4 && mkdir -p variants
echo "GPU Variant Calling" > variants/output.vcf
echo "Threads: 4, Memory: 32G" >> variants/output.vcf
nvidia-smi >> variants/output.vcf 2>&1 || echo "GPU not available" >> variants/output.vcf
