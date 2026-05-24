# Plugin System

oxo-flow supports a compile-time + config-based plugin architecture. Plugins
are Rust crates implementing standard traits, registered via TOML configuration
files. Plugin authenticity is verified using HMAC-SHA256 signatures.

## Quick Start

### 1. Implement a plugin trait

```rust
use oxo_flow_core::plugin::RulePlugin;
use oxo_flow_core::rule::Rule;
use oxo_flow_core::error::Result;
use std::collections::HashMap;

struct MyPlugin;
impl RulePlugin for MyPlugin {
    fn rule_type(&self) -> &str { "my-custom-type" }
    fn build_command(&self, rule: &Rule, values: &HashMap<String, String>) -> Result<String> {
        Ok(format!("custom_tool --input {}", rule.input[0]))
    }
    fn validate(&self, rule: &Rule) -> Result<()> { Ok(()) }
}
```

### 2. Register with the registry

```rust
use oxo_flow_core::plugin::PluginRegistry;

let mut registry = PluginRegistry::default();
registry.register_rule(Box::new(MyPlugin));
registry.add_trusted_key("key-001", "your-secret-key");
```

### 3. Declare in your workflow

```toml
[plugins]
rules = ["my-custom-type"]
executor = "slurm-custom"
reports = ["native-pdf"]
trusted_keys_file = ".oxo-flow/trusted_keys.toml"
```

## Available Traits

| Trait | Purpose | Key Method |
|-------|---------|------------|
| `RulePlugin` | Custom rule types | `build_command()` |
| `ExecutorPlugin` | Custom executors | `submit()` |
| `ReportPlugin` | Custom report renderers | `render()` |

## Plugin Discovery

Plugins are discovered from `.plugin.toml` files in:

- `~/.oxo-flow/plugins/` — user-level (shared across projects)
- `<project>/.oxo-flow/plugins/` — project-level

```toml
# my-plugin.plugin.toml
name = "my-custom-type"
version = "1.0.0"
plugin_type = "rule"
description = "Custom rule for specialized analysis"
author = "Your Name"
command_template = "custom_tool {input} > {output}"

[signature]
key_id = "key-001"
value = "a1b2c3d4..."
```

## Signature Verification

Each plugin manifest can include a `[signature]` section with an HMAC-SHA256
digest. The registry verifies signatures against trusted keys before loading:

```rust
registry.add_trusted_key("key-001", "shared-secret-key");
registry.discover(Some(project_dir))?; // verified plugins only
```

## API Reference

### `PluginRegistry`

```rust
impl PluginRegistry {
    pub fn register_rule(&mut self, plugin: Box<dyn RulePlugin>);
    pub fn register_executor(&mut self, plugin: Box<dyn ExecutorPlugin>);
    pub fn register_report(&mut self, plugin: Box<dyn ReportPlugin>);
    pub fn add_trusted_key(&mut self, key_id: &str, key: &str);
    pub fn discover(&mut self, project_dir: Option<&Path>) -> Result<usize>;
    pub fn find_rule(&self, rule_type: &str) -> Option<&dyn RulePlugin>;
    pub fn find_executor(&self, backend: &str) -> Option<&dyn ExecutorPlugin>;
    pub fn find_report(&self, renderer: &str) -> Option<&dyn ReportPlugin>;
}
```

### `PluginsConfig` (TOML `[plugins]` section)

```rust
pub struct PluginsConfig {
    pub rules: Vec<String>,        // Rule plugin types to enable
    pub executor: Option<String>,  // Executor plugin to use
    pub reports: Vec<String>,      // Report plugins to enable
    pub trusted_keys_file: Option<String>, // Path to keys file
}
```

## See Also

- [Plugin module source](https://github.com/Traitome/oxo-flow/blob/main/crates/oxo-flow-core/src/plugin.rs)
- [Rule reference](workflow-format.md)
- [ROADMAP.md](https://github.com/Traitome/oxo-flow/blob/main/ROADMAP.md)
