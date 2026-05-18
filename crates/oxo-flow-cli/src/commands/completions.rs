//! Logic for the 'completions' command.

use crate::Cli;
use anyhow::Result;
use clap::CommandFactory;
use clap_complete::{Shell, generate};

pub fn handle_completions(shell: Shell) -> Result<()> {
    let mut cmd = Cli::command();
    let name = cmd.get_name().to_string();
    generate(shell, &mut cmd, name, &mut std::io::stdout());
    Ok(())
}
