# Hardware Scaling Evidence Status

> **Status**: `MODEL PROJECTION`, not measurement.
>
> **Experimental result**: none.
>
> **Important**: no number from the projection path may be used as evidence of real speedup, NUMA benefit, energy savings, or publication-tier performance.

## 1. Current Evidence

The repository contains a deterministic hardware-scaling model and a separate path for ingesting raw run-level measurements.

The projection path:

- uses fixed assumed speedup factors;
- prints `MODEL PROJECTION` and `NOT MEASUREMENT`;
- is available only with `--allow-simulation`;
- has no inferential status.

The measured path:

- requires `--measurements-json`;
- requires host and target topology metadata;
- requires unique run IDs, seeds, and at least two runs per configuration;
- computes confidence intervals from run-level samples.

Only the measured path can support an empirical hardware claim.

## 2. Required Measurement Manifest

Each measured campaign must record:

- source commit and build profile;
- operating system, kernel, CPU model, sockets, NUMA nodes, cache topology, and memory;
- storage devices, filesystems, mount options, and network path;
- workload, cohort size, input checksums, and random seeds;
- scheduler configuration and affinity policy;
- tool versions and container or environment digests;
- warm-up policy, repetitions, exclusions, and raw per-run telemetry;
- wall time, throughput, peak memory, I/O, CPU, cache/NUMA counters where available, energy, and failure status;
- command transcript and generated configuration.

## 3. Required Analysis

- Pre-register the primary performance outcome.
- Report all runs, not only the best run.
- Report medians or means with confidence intervals and dispersion.
- Separate scheduling, I/O, NUMA, and scratch-tier effects with controlled ablations.
- Test fallback and degradation, including single-socket, constrained-memory, and slow-storage hosts.
- Reanalyze raw data independently.
- Report negative results.

## 4. Current Decision

Hardware performance remains **Planned / blocked**. The next admissible artifact is a measured campaign manifest and raw run-level data, not a speedup table.
