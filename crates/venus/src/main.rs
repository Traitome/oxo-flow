#![forbid(unsafe_code)]
//! Venus CLI — Clinical-grade tumor variant detection pipeline generator.
//!
//! Generates `.oxoflow` workflow files for tumor variant analysis.

use anyhow::{Context, Result};
use std::path::PathBuf;

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();

    if args.len() < 2 || args[1] == "--help" || args[1] == "-h" {
        print_help();
        return Ok(());
    }

    if args[1] == "--version" || args[1] == "-V" {
        println!("venus {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }

    match args[1].as_str() {
        "generate" => cmd_generate(&args[2..]),
        "validate" => cmd_validate(&args[2..]),
        "list-steps" => cmd_list_steps(),
        other => {
            eprintln!("Unknown command: {other}");
            eprintln!("Run 'venus --help' for usage information.");
            std::process::exit(1);
        }
    }
}

fn print_help() {
    println!(
        "\
venus {} — Clinical-grade tumor variant detection pipeline generator

USAGE:
    venus <COMMAND> [OPTIONS]

COMMANDS:
    generate     Generate a .oxoflow workflow file from a Venus configuration
    validate     Validate a Venus configuration TOML file
    list-steps   List all available pipeline steps

OPTIONS:
    -h, --help       Print help information
    -V, --version    Print version information

EXAMPLES:
    venus generate config.toml -o pipeline.oxoflow
    venus validate config.toml
    venus list-steps",
        env!("CARGO_PKG_VERSION")
    );
}

fn cmd_generate(args: &[String]) -> Result<()> {
    if args.is_empty() {
        anyhow::bail!("Usage: venus generate <config.toml> [-o output.oxoflow]");
    }

    let config_path = PathBuf::from(&args[0]);
    let output_path = if args.len() >= 3 && args[1] == "-o" {
        PathBuf::from(&args[2])
    } else {
        config_path.with_extension("oxoflow")
    };

    let content =
        std::fs::read_to_string(&config_path).context("Failed to read configuration file")?;

    let config: oxo_flow_venus::VenusConfig =
        toml::from_str(&content).context("Failed to parse Venus configuration")?;

    config
        .validate()
        .context("Venus configuration validation failed")?;

    let oxoflow_toml =
        oxo_flow_venus::generate_oxoflow(&config).context("Failed to generate .oxoflow content")?;

    std::fs::write(&output_path, &oxoflow_toml)
        .with_context(|| format!("Failed to write output to {}", output_path.display()))?;

    println!("Generated pipeline: {}", output_path.display());
    Ok(())
}

fn cmd_validate(args: &[String]) -> Result<()> {
    if args.is_empty() {
        anyhow::bail!("Usage: venus validate <config.toml>");
    }

    let config_path = PathBuf::from(&args[0]);
    let content =
        std::fs::read_to_string(&config_path).context("Failed to read configuration file")?;

    let config: oxo_flow_venus::VenusConfig =
        toml::from_str(&content).context("Failed to parse Venus configuration")?;

    let total_samples = config.tumor_samples.len() + config.normal_samples.len();

    match config.validate() {
        Ok(()) => {
            println!("✓ Configuration is valid");
            println!("  Mode: {}", config.mode);
            println!("  Sequencing: {}", config.seq_type);
            println!("  Genome: {}", config.genome_build);
            println!("  Samples: {}", total_samples);
            Ok(())
        }
        Err(e) => {
            eprintln!("✗ Validation failed: {e}");
            std::process::exit(1);
        }
    }
}

fn cmd_list_steps() -> Result<()> {
    println!("Venus Pipeline Steps:");
    println!();
    println!("  1. Fastp           — FASTQ quality control and trimming");
    println!("  2. BwaMem2         — Read alignment to reference genome");
    println!("  3. MarkDuplicates  — PCR duplicate marking");
    println!("  4. Bqsr            — Base quality score recalibration");
    println!("  5. HaplotypeCaller — Germline variant calling (GATK)");
    println!("  6. Mutect2         — Somatic variant calling (GATK)");
    println!("  7. FilterMutect    — Somatic variant filtering");
    println!("  8. Strelka2        — Somatic variant calling (paired mode only)");
    println!("  9. CnvKit          — Copy number variant detection");
    println!(" 10. MsiSensor       — Microsatellite instability analysis");
    println!(" 11. TmbCalc         — Tumor mutation burden calculation");
    println!(" 12. Vep             — Variant effect prediction / annotation");
    println!(" 13. ClinicalReport  — Clinical report generation");
    Ok(())
}
