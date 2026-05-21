#!/bin/bash
#BSUB -J deep_learning
#BSUB -n 16
#BSUB -M 64G
#BSUB -gpu 4
#BSUB -q workq
#BSUB -o logs/deep_learning.out
#BSUB -e logs/deep_learning.err

mkdir -p logs
module load cuda/12.0 pytorch/2.0 && mkdir -p models
echo "Deep Learning Model" > models/model.pt
echo "GPUs: 4, Memory: 64G" >> models/model.pt
echo "Training complete" >> models/model.pt
