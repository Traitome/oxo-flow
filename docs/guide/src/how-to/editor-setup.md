# Editor Setup for `.oxoflow` Files

Workflow files (`.oxoflow`) are plain [TOML](https://toml.io/) — oxo-flow's
declarative workflow format. Most editors do not recognize the `.oxoflow`
extension yet, so configure one small file-type association and you get full
TOML support: syntax highlighting, bracket matching, folding, and formatting.

## VS Code: the official extension (recommended)

In VS Code, skip the manual association entirely — install the
**oxo-flow Pipeline** extension (`traitome.oxo-flow`):

```bash
code --install-extension traitome.oxo-flow
```

or search *oxo-flow* in the Extensions view (also available on
[Open VSX](https://open-vsx.org/extension/traitome/oxo-flow) for VSCodium,
Cursor, and other forks). It provides TOML+ syntax highlighting,
schema-driven completion with hover docs, background `validate`/`lint`
diagnostics, canonical formatting, one-click run/dry-run/resume, and AI
pipeline generation — see the [VS Code Extension](../reference/vscode-extension.md)
reference. The rest of this page covers manual setups and other editors.

## VS Code (without the extension)

### Workspace (per repository)

The [oxo-flow-community](https://github.com/oxo-flow-community) workflow
repositories ship a committed `.vscode/settings.json` with this content. In
the oxo-flow repository itself `.vscode/` is git-ignored, so create the file
in the folder you work in:

```json
{
  "files.associations": {
    "*.oxoflow": "toml"
  }
}
```

Open the folder in VS Code and `.oxoflow` files highlight as TOML
immediately — nothing to install.

### User-level (all folders, all projects)

To apply the association everywhere, open the command palette
(`Cmd/Ctrl+Shift+P`) → *Preferences: Open User Settings (JSON)* and add the
same `files.associations` block. Alternatively: *Settings* → search
`files.associations` → *Add Item* with key `*.oxoflow` and value `toml`.

## Other editors

### Zed

Add to `~/.config/zed/settings.json`:

```json
{
  "file_types": {
    "TOML": ["oxoflow"]
  }
}
```

### Helix

Add `"oxoflow"` to the `toml` language's `file-types` in `~/.config/helix/languages.toml`:

```toml
[[language]]
name = "toml"
file-types = ["toml", "oxoflow"]
```

### Neovim

```lua
vim.filetype.add({ extension = { oxoflow = "toml" } })
```

### JetBrains IDEs (IntelliJ / RustRover / PyCharm)

*Settings* → *Editor* → *File Types*, select **TOML**, and add the
`*.oxoflow` pattern in the *File name patterns* list.

### Sublime Text

Open an `.oxoflow` file → *View* → *Syntax* → *Open all with current
extension as...* → **TOML**.

### Emacs

```elisp
(add-to-list 'auto-mode-alist '("\\.oxoflow\\'" . toml-mode))
```

## Optional: schema-backed validation and completion

For key-name validation and completion while editing, dump the JSON Schema
reference and associate it in VS Code with the
[Even Better TOML](https://marketplace.visualstudio.com/items?itemName=tamasfe.even-better-toml)
extension's settings:

```bash
oxo-flow schema > oxo-flow-schema.json
```

```json
{
  "evenBetterToml.schema.associations": {
    "*.oxoflow": "./oxo-flow-schema.json"
  }
}
```

Note: `oxo-flow schema` exports a subset of the full format — the complete
reference lives in the
[Workflow Format](../reference/workflow-format.md) documentation.

## See also

- [VS Code Extension](../reference/vscode-extension.md)
- [Create a Workflow](create-workflow.md)
- [Workflow Format](../reference/workflow-format.md)
