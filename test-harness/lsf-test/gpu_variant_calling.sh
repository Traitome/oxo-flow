#!/bin/bash
#BSUB -J gpu_variant_calling
#BSUB -n 4
#BSUB -M 32G
#BSUB -gpu 1
#BSUB -q workq
#BSUB -o logs/gpu_variant_calling.out
#BSUB -e logs/gpu_variant_calling.err

mkdir -p logs
module load cuda/12.0 gatk/4.4 && mkdir -p variants
echo "GPU Variant Calling" > variants/output.vcf
echo "Threads: 4, Memory: 32G" >> variants/output.vcf
nvidia-smi >> variants/output.vcf 2>&1 || echo "GPU not available" >> variants/output.vcf
