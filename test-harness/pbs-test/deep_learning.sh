#!/bin/bash
#PBS -N deep_learning
#PBS -l nodes=1:ppn=16,mem=64G,gpu=4
#PBS -q workq
#PBS -o logs/deep_learning.out
#PBS -e logs/deep_learning.err

mkdir -p logs
module load cuda/12.0 pytorch/2.0 && mkdir -p models
echo "Deep Learning Model" > models/model.pt
echo "GPUs: 4, Memory: 64G" >> models/model.pt
echo "Training complete" >> models/model.pt
