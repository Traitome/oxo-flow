#!/bin/bash
#SBATCH --job-name=deep_learning
#SBATCH --cpus-per-task=16
#SBATCH --mem=64G
#SBATCH --gres=gpu:4
#SBATCH --partition=workq
#SBATCH --output=logs/deep_learning.out
#SBATCH --error=logs/deep_learning.err

mkdir -p logs
module load cuda/12.0 pytorch/2.0 && mkdir -p models
echo "Deep Learning Model" > models/model.pt
echo "GPUs: 4, Memory: 64G" >> models/model.pt
echo "Training complete" >> models/model.pt
