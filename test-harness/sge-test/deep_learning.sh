#!/bin/bash
#$ -N deep_learning
#$ -pe smp 16
#$ -l h_vmem=64G
#$ -l gpu=4
#$ -q workq
#$ -o logs/deep_learning.out
#$ -e logs/deep_learning.err

mkdir -p logs
module load cuda/12.0 pytorch/2.0 && mkdir -p models
echo "Deep Learning Model" > models/model.pt
echo "GPUs: 4, Memory: 64G" >> models/model.pt
echo "Training complete" >> models/model.pt
