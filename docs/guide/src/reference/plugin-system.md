# Plugin System

> **Status: Trait definitions implemented, dynamic loading planned for v0.8+.**
> The core traits (`RulePlugin`, `ExecutorPlugin`, `ReportPlugin`) are defined
> in `oxo-flow-core::plugin` and ready for experimentation. Dynamic loading
> (`libloading`), filesystem discovery, and signature verification are not yet
> implemented.

## What's Available Now (v0.6)

```rust
use oxo_flow_core::plugin::{PluginRegistry, RulePlugin, ExecutorPlugin, ReportPlugin};

let mut registry = PluginRegistry::default();
// Implement the traits and register your plugin
registry.register_rule(Box::new(MyCustomRule));
```

### Defined Traits

| Trait | Location | Purpose |
|-------|----------|---------|
| `RulePlugin` | `crates/oxo-flow-core/src/plugin.rs` | Custom rule types (R, Python, Jupyter) |
| `ExecutorPlugin` | same | Custom executors (AWS Batch, K8s, GCP) |
| `ReportPlugin` | same | Custom report renderers (PDF, DOCX) |
| `PluginRegistry` | same | Plugin collection and registration |

All traits are usable today — implement them in your own code and pass instances
to the registry. Integration with the `oxo-flow run` executor is planned for v0.7.

## Future Plans

- **Dynamic loading** (`v0.8`): Load `.so`/`.dylib` plugins from
  `~/.oxo-flow/plugins/` at runtime via `libloading`.
- **Plugin discovery** (`v0.8`): Auto-scan plugin directories for registered
  extension points.
- **TOML integration** (`v0.7`): Declare plugin usage in `.oxoflow` files:
  ```toml
  [plugins]
  rules = ["r-function"]
  executor = "aws-batch"
  ```
- **Signature verification** (`v0.9`): ed25519 plugin signing.
- **Plugin registry** (`v1.0`): Community plugin marketplace.

See [ROADMAP.md](https://github.com/Traitome/oxo-flow/blob/main/ROADMAP.md) for timelines.
