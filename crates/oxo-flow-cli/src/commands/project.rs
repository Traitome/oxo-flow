use anyhow::Result;
use colored::Colorize;
use std::path::PathBuf;

use crate::commands::print_banner;

pub fn init_command(name: String, dir: Option<PathBuf>) -> Result<()> {
    print_banner();
    let project_dir = dir.unwrap_or_else(|| PathBuf::from(&name));
    std::fs::create_dir_all(&project_dir)?;

    let workflow_content = format!(
        r#"[workflow]
name = "{name}"
version = "0.1.0"
description = "A new oxo-flow pipeline"

[config]
# Variables defined here can be used in shell commands as {{config.key}}
sample_name = "example"

[defaults]
threads = 1
memory = "1G"

[[rules]]
name = "hello_world"
input = ["data/input.txt"]
output = ["results/{{config.sample_name}}_output.txt"]
# Double braces are used to reference wildcards or config variables
shell = "cat {{input[0]}} > {{output[0]}} && echo 'Hello from oxo-flow!' >> {{output[0]}}"
"#
    );

    let workflow_path = project_dir.join(format!("{name}.oxoflow"));
    std::fs::write(&workflow_path, workflow_content)?;

    // Create additional directories
    let envs_dir = project_dir.join("envs");
    let scripts_dir = project_dir.join("scripts");
    let data_dir = project_dir.join("data");
    let results_dir = project_dir.join("results");
    std::fs::create_dir_all(&envs_dir)?;
    std::fs::create_dir_all(&scripts_dir)?;
    std::fs::create_dir_all(&data_dir)?;
    std::fs::create_dir_all(&results_dir)?;

    // Create initial input file
    std::fs::write(
        data_dir.join("input.txt"),
        "This is your starting input data.\n",
    )?;

    // Create starter environment file
    let env_content = "\
# Example Conda environment specification
name: example-env
channels:
  - bioconda
  - conda-forge
  - defaults
dependencies:
  - fastp=0.23.4
  - samtools=1.18
";
    std::fs::write(envs_dir.join("example.yaml"), env_content)?;

    // Create starter script
    let script_content = "\
#!/bin/bash
# Example helper script
echo \"Running helper script for $1\"
";
    std::fs::write(scripts_dir.join("example.sh"), script_content)?;

    // Create a .gitignore with common bioinformatics patterns
    let gitignore_content = "\
# Alignment files
*.bam
*.bam.bai
*.cram
*.cram.crai
*.sam

# Variant files
*.vcf.gz
*.vcf.gz.tbi
*.bcf

# Index files
*.fai
*.dict

# Workflow outputs
logs/
results/
benchmarks/

# oxo-flow internals
.oxo-flow/
.oxo-flow-cache/

# OS files
.DS_Store
Thumbs.db
";
    let gitignore_path = project_dir.join(".gitignore");
    std::fs::write(&gitignore_path, gitignore_content)?;

    eprintln!(
        "{} Created new project at {}",
        "✓".green().bold(),
        project_dir.display()
    );
    eprintln!("  {}", workflow_path.display());
    eprintln!("  {}/example.yaml", envs_dir.display());
    eprintln!("  {}/example.sh", scripts_dir.display());
    eprintln!("  {}", gitignore_path.display());
    eprintln!(
        "\n  {} To run your first workflow:",
        "Next steps:".bold().cyan()
    );
    eprintln!("    cd {}", project_dir.display());
    eprintln!(
        "    oxo-flow run {}",
        workflow_path.file_name().unwrap().to_str().unwrap()
    );

    Ok(())
}

pub fn template_command(_name: Option<String>) -> Result<()> {
    print_banner();
    eprintln!(
        "{} The 'template' command is not yet implemented.",
        "Note:".bold().cyan()
    );
    eprintln!("  Try 'oxo-flow init' to create a basic workflow structure.");
    Ok(())
}
