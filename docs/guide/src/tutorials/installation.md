# Installation

This guide covers all the ways to install the `oxo-flow` binary on your system.

---

## Requirements

- **Operating system**: Linux (x86_64, aarch64) or macOS (Apple Silicon, Intel)
- **Disk space**: ~23 MB for the binary
- **Optional**: Rust toolchain (1.98+) if building from source

!!! note "Runtime dependencies"
    oxo-flow itself has no runtime dependencies — it is a single static binary. However, the *tools your workflows call* (e.g., `bwa`, `samtools`, `GATK`) must be available either on your `$PATH` or through an environment manager (conda, docker, etc.) declared in your `.oxoflow` file.

---

## Option 1 — Install with Cargo (recommended)

=== "Install Rust Toolchain First"

    If you don't have Rust installed, use the official installer:

    ```bash
    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
    source "$HOME/.cargo/env"
    ```

=== "Install oxo-flow"

    ```bash
    cargo install oxo-flow-cli
    ```

    This builds the latest published release and places the `oxo-flow` binary in `~/.cargo/bin/`.

    **Verify PATH**: Ensure `~/.cargo/bin/` is in your `$PATH`. You can check with:
    ```bash
    echo $PATH | grep -q ".cargo/bin" || echo 'Add to PATH: export PATH="$HOME/.cargo/bin:$PATH"'
    ```

Verify the installation:

```bash
oxo-flow --version
# oxo-flow 0.17.2
```

!!! tip "Updating"
    Run the same `cargo install oxo-flow-cli` command to update to the latest version. Cargo will rebuild if a newer version is available.

---

## Option 2 — Build from Source

Clone the repository and build the workspace:

```bash
git clone https://github.com/Traitome/oxo-flow.git
cd oxo-flow
cargo build --release --workspace
```

!!! note "`--workspace` is required"
    The repo root is also a library package (the integration tests). A bare
    `cargo build --release` compiles only that package and produces **no**
    `target/release/oxo-flow` — pass `--workspace` to build every crate.

The binary is at `target/release/oxo-flow`. Copy it to a directory on your `$PATH`:

```bash
cp target/release/oxo-flow ~/.local/bin/
```

### Development build

For faster compile times during development (without optimizations):

```bash
cargo build --workspace
# Binary at: target/debug/oxo-flow
```

---

## Option 3 — Download Pre-built Binary

Pre-built binaries are available from the [GitHub Releases](https://github.com/Traitome/oxo-flow/releases) page.

=== "Linux (x86_64)"

    ```bash
    curl -LO https://github.com/Traitome/oxo-flow/releases/download/v0.17.2/oxo-flow-v0.17.2-x86_64-unknown-linux-gnu.tar.gz
    tar xzf oxo-flow-v0.17.2-x86_64-unknown-linux-gnu.tar.gz
    chmod +x oxo-flow
    mv oxo-flow ~/.local/bin/
    ```

=== "macOS (Apple Silicon)"

    ```bash
    curl -LO https://github.com/Traitome/oxo-flow/releases/download/v0.17.2/oxo-flow-v0.17.2-aarch64-apple-darwin.tar.gz
    tar xzf oxo-flow-v0.17.2-aarch64-apple-darwin.tar.gz
    chmod +x oxo-flow
    mv oxo-flow /usr/local/bin/  # Or another folder in your PATH
    ```

**Verify the download** against `SHA256SUMS.txt` (published with every
release):

```bash
curl -LO https://github.com/Traitome/oxo-flow/releases/download/v0.17.2/SHA256SUMS.txt
sha256sum -c SHA256SUMS.txt --ignore-missing
```

**Other targets**: `gnu` builds need glibc, `musl` builds are
statically linked (Alpine, containers), and `armv7` covers 32-bit ARM.
Desktop users may prefer the `.deb` / `.rpm` / `.AppImage` /
`.dmg` bundles — see [Desktop App Packaging](../how-to/desktop-app.md).

---

## Option 4 — Run with Docker

Images are published to GitHub Container Registry automatically on every release and every push to `main`.
Release images are multi-arch (`linux/amd64` + `linux/arm64`) and are assembled from the
SHA256-verified release binaries. `:latest` moves only after the release image passes a health
smoke test, `:<major.minor>` (e.g. `:0.17`) tracks the newest patch of a minor line, and `:main`
is a multi-arch dev build compiled from source:

```bash
# Web UI at http://localhost:3000
docker run -d --name oxo-flow -p 3000:3000 ghcr.io/traitome/oxo-flow:latest

# Pin a specific release (or track a minor line)
docker run -d -p 3000:3000 ghcr.io/traitome/oxo-flow:0.17.2
docker run -d -p 3000:3000 ghcr.io/traitome/oxo-flow:0.17

# CLI one-shot usage — mount your workflow directory
docker run --rm -v "$PWD:/work" -w /work ghcr.io/traitome/oxo-flow:latest \
  oxo-flow run my-pipeline.oxoflow
```

!!! note "Data persistence"
    The server stores its database in `/app/data` inside the container. Mount a
    volume to keep it across restarts: `-v oxo-flow-data:/app/data`. The image
    runs as UID 1000 — make sure the mounted host directory is writable by that
    user.

---

## Optional Dependencies

### Graphviz (for Visualization)

The `oxo-flow graph` command outputs workflows in [DOT format](https://graphviz.org/doc/info/lang.html). To render these graphs as images (PNG, SVG, etc.), you need to install **Graphviz**.

=== "macOS"

    ```bash
    brew install graphviz
    ```

=== "Linux (Ubuntu/Debian)"

    ```bash
    sudo apt install graphviz
    ```

=== "Conda"

    ```bash
    conda install -c conda-forge graphviz
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
✓ my-test-pipeline.oxoflow — 1 rules, 0 dependencies
```

---

## Next Steps

- [Quick Start](./quickstart.md) — run a workflow in 5 minutes
- [Your First Workflow](./first-workflow.md) — build a pipeline from scratch
