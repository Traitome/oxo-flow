# Install the oxo-flow CLI

The extension drives the `oxo-flow` binary for everything it does —
validation, lint, formatting, running. Without it the editor still
highlights and completes, but the status bar shows
`$(warning) oxo-flow`.

Easiest (Rust toolchain required):

```bash
cargo install oxo-flow-cli
```

The binary lands in `~/.cargo/bin/` — make sure that directory is on
your `PATH` (`oxo-flow --version` should print a version).

Other ways in — prebuilt release binaries, Docker images, and building
from source — are in the [installation guide](https://traitome.github.io/oxo-flow/latest/tutorials/installation/).

Already installed but not found? Click the status bar item (or run
**oxo-flow: Open Settings**) and set **oxo-flow.executablePath** to the
absolute path of the binary.
