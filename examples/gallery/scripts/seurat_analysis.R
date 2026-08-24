#!/usr/bin/env Rscript
# Seurat clustering + UMAP for gallery 09 (single-cell RNA-seq).
#
# Reads a CellRanger filtered feature-barcode matrix (HDF5), runs the
# standard QC -> normalize -> PCA -> UMAP -> clustering pipeline, and
# writes a Seurat object (RDS) plus a UMAP plot into --output-dir.
#
# Requirements: r-seurat >= 5 (see gallery envs/seurat.yaml).

args <- commandArgs(trailingOnly = TRUE)

parse_arg <- function(name) {
  idx <- match(name, args)
  if (is.na(idx)) {
    stop("missing required argument: ", name)
  }
  if (idx == length(args) || startsWith(args[idx + 1], "--")) {
    stop("argument ", name, " is missing its value")
  }
  args[idx + 1]
}

input <- parse_arg("--input")
output_dir <- parse_arg("--output-dir")

suppressPackageStartupMessages(library(Seurat))

counts <- Read10X_h5(input)
seu <- CreateSeuratObject(counts = counts, min.cells = 3, min.features = 200)
seu[["percent.mt"]] <- PercentageFeatureSet(seu, pattern = "^MT-")
seu <- subset(seu, subset = nFeature_RNA > 200 & percent.mt < 5)

seu <- NormalizeData(seu)
seu <- FindVariableFeatures(seu, selection.method = "vst", nfeatures = 2000)
seu <- ScaleData(seu)
seu <- RunPCA(seu, npcs = 30)
seu <- RunUMAP(seu, dims = 1:30)
seu <- FindNeighbors(seu, dims = 1:30)
seu <- FindClusters(seu, resolution = 0.5)

dir.create(output_dir, recursive = TRUE, showWarnings = FALSE)
saveRDS(seu, file = file.path(output_dir, "seurat_object.rds"))
png(file.path(output_dir, "umap_plot.png"), width = 800, height = 600)
print(DimPlot(seu, reduction = "umap", label = TRUE))
invisible(dev.off())
