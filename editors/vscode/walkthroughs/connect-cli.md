# Connect the extension to the CLI

If the status bar shows `$(warning) oxo-flow`, the binary wasn't found
on `PATH`. Fix it in one of two ways:

1. **Extend `PATH`** so `oxo-flow --version` works in any terminal
   (e.g. `export PATH="$HOME/.cargo/bin:$PATH"` in your shell profile).
2. **Set the absolute path** in the extension setting:
   [oxo-flow: Open Settings](command:oxo-flow.openSettings) → edit
   **Oxo Flow › Executable Path** (e.g. `~/.cargo/bin/oxo-flow`).

The setting is *machine-overridable*, so on SSH remotes and
devcontainers each machine can point at its own binary — the extension
runs on the remote where your pipelines live.

Once the status bar shows the version (e.g. `$(circle-filled) oxo-flow
0.22.0`), everything below — diagnostics, formatting, run — lights up.
