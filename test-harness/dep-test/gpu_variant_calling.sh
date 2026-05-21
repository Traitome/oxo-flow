#!/bin/bash
#SBATCH --job-name=gpu_variant_calling
#SBATCH --cpus-per-task=4
#SBATCH --mem=32G
#SBATCH --gres=gpu:1
#SBATCH --partition=workq
#SBATCH --output=logs/gpu_variant_calling.out
#SBATCH --error=logs/gpu_variant_calling.err

mkdir -p logs
module load cuda/12.0 gatk/4.4 && mkdir -p variants
echo "GPU Variant Calling" > variants/output.vcf
echo "Threads: 4, Memory: 32G" >> variants/output.vcf
nvidia-smi >> variants/output.vcf 2>&1 || echo "GPU not available" >> variants/output.vcf
