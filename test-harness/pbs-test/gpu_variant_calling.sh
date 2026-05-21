#!/bin/bash
#PBS -N gpu_variant_calling
#PBS -l nodes=1:ppn=4,mem=32G,gpu=1
#PBS -q workq
#PBS -o logs/gpu_variant_calling.out
#PBS -e logs/gpu_variant_calling.err

mkdir -p logs
module load cuda/12.0 gatk/4.4 && mkdir -p variants
echo "GPU Variant Calling" > variants/output.vcf
echo "Threads: 4, Memory: 32G" >> variants/output.vcf
nvidia-smi >> variants/output.vcf 2>&1 || echo "GPU not available" >> variants/output.vcf
