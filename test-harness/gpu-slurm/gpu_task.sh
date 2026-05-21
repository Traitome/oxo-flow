#!/bin/bash
#SBATCH --job-name=gpu_task
#SBATCH --cpus-per-task=4
#SBATCH --mem=16G
#SBATCH --gres=gpu:1
#SBATCH --partition=workq
#SBATCH --output=logs/gpu_task.out
#SBATCH --error=logs/gpu_task.err

mkdir -p logs
module purge
module load cuda
module load cuda && echo "Running on GPU" > gpu_output.txt
echo "Threads: 4" >> gpu_output.txt
echo "Memory: 16G" >> gpu_output.txt
nvidia-smi >> gpu_output.txt
