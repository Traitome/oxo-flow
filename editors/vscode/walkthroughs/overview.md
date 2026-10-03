# Welcome to oxo-flow

A pipeline is a **DAG of rules**: each `[[rules]]` block declares inputs,
outputs, and a command, and the engine runs them in dependency order,
skipping anything already up to date.

`.oxoflow` files are plain TOML with superpowers — wildcards
(`{sample}`), 8 environment backends, and checkpoint/resume. The editor
gives you schema-driven completion (place the cursor inside a value and
press <kbd>Ctrl+Space</kbd>), hover docs, and background validation as
soon as the CLI is installed.

**Next:** get the `oxo-flow` CLI on your machine → *Install the CLI*.
