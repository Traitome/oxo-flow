# Installation

This guide covers all the ways to install the `oxo-flow` binary on your system.

---

## Requirements

- **Operating system**: Linux (x86_64, aarch64) or macOS (Apple Silicon, Intel)
- **Disk space**: ~50 MB for the binary
- **Optional**: Rust toolchain (1.85+) if building from source

!!! note "Runtime dependencies"
    oxo-flow itself has no runtime dependencies — it is a single static binary. However, the *tools your workflows call* (e.g., `bwa`, `samtools`, `GATK`) must be available either on your `$PATH` or through an environment manager (conda, docker, etc.) declared in your `.oxoflow` file.

---

## Option 1 — Install with Cargo (recommended)

If you have the Rust toolchain installed:

```bash
cargo install oxo-flow
```

This builds the latest published release and places the `oxo-flow` binary in `~/.cargo/bin/`.

Verify the installation:

```bash
oxo-flow --version
# oxo-flow 0.1.0
```

!!! tip "Updating"
    Run the same `cargo install oxo-flow` command to update to the latest version. Cargo will rebuild if a newer version is available.

---

## Option 2 — Build from Source

Clone the repository and build the workspace:

```bash
git clone https://github.com/Traitome/oxo-flow.git
cd oxo-flow
cargo build --release
```

The binary is at `target/release/oxo-flow`. Copy it to a directory on your `$PATH`:

```bash
cp target/release/oxo-flow ~/.local/bin/
```

### Development build

For faster compile times during development (without optimizations):

```bash
cargo build
# Binary at: target/debug/oxo-flow
```

---

## Option 3 — Download Pre-built Binary

Pre-built binaries are available from the [GitHub Releases](https://github.com/Traitome/oxo-flow/releases) page.

```bash
# Example for Linux x86_64
curl -LO https://github.com/Traitome/oxo-flow/releases/latest/download/oxo-flow-linux-x86_64.tar.gz
tar xzf oxo-flow-linux-x86_64.tar.gz
chmod +x oxo-flow
mv oxo-flow ~/.local/bin/
```

---

## Shell Completions

oxo-flow can generate shell completions for Bash, Zsh, Fish, Elvish, and PowerShell:

=== "Bash"

    ```bash
    oxo-flow completions bash > ~/.local/share/bash-completion/completions/oxo-flow
    ```

=== "Zsh"

    ```bash
    oxo-flow completions zsh > ~/.zfunc/_oxo-flow
    # Add to .zshrc: fpath+=~/.zfunc && autoload -Uz compinit && compinit
    ```

=== "Fish"

    ```bash
    oxo-flow completions fish > ~/.config/fish/completions/oxo-flow.fish
    ```

---

## Verify Installation

After installation, confirm everything is working:

```bash
# Check version
oxo-flow --version

# Show help
oxo-flow --help

# Initialize a test project
oxo-flow init my-test-pipeline
cd my-test-pipeline
oxo-flow validate my-test-pipeline.oxoflow
```

Expected output:

```
✓ my-test-pipeline.oxoflow — 0 rules, 0 dependencies
```

---

## Next Steps

- [Quick Start](./quickstart.md) — run a workflow in 5 minutes
- [Your First Workflow](./first-workflow.md) — build a pipeline from scratch
