# System Architecture

oxo-flow is organized as a Cargo workspace with four crates that form a layered architecture.

---

## Workspace Layout

```
oxo-flow/
├── crates/
│   ├── oxo-flow-core/    # Core library
│   ├── oxo-flow-cli/     # CLI binary
│   ├── oxo-flow-web/     # Web API server
│   └── venus/            # Clinical pipeline
├── pipelines/            # Pipeline definitions
├── examples/             # Example workflows
└── tests/                # Integration tests
```

---

## Crate Dependencies

```mermaid
graph TD
    CLI[oxo-flow-cli] --> Core[oxo-flow-core]
    CLI --> Web[oxo-flow-web]
    Web --> Core
    Venus[oxo-flow-venus] --> Core
```

- **oxo-flow-core** is the foundation — all other crates depend on it
- **oxo-flow-cli** is the user-facing binary that ties everything together
- **oxo-flow-web** provides the REST API layer on top of core
- **oxo-flow-venus** is a domain-specific pipeline crate built on core

---

## Core Library Modules

The `oxo-flow-core` crate is organized into focused modules:

| Module | Responsibility |
|---|---|
| `config` | Parse `.oxoflow` TOML files into `WorkflowConfig` |
| `rule` | Rule definitions: inputs, outputs, shell, resources, environment |
| `dag` | Build and validate the dependency DAG, topological sorting |
| `executor` | Execute rules locally with checkpointing, resource enforcement |
| `scheduler` | Resource-aware job scheduling with ResourcePool |
| `environment` | Resolve and activate conda, docker, singularity, pixi, venv; cache setup state |
| `wildcard` | Expand `{sample}` patterns in file paths |
| `report` | Generate HTML and JSON reports from templates |
| `container` | Generate Dockerfile and Singularity definitions |
| `cluster` | Generate SLURM, PBS, SGE, LSF job scripts with environment wrapping |
| `error` | Unified error types (`OxoFlowError`) |
| `format` | Output formatting utilities |

---

## Data Flow

A typical workflow execution follows this path:

```mermaid
sequenceDiagram
    participant User
    participant CLI
    participant Config
    participant DAG
    participant Scheduler
    participant Executor
    participant ResourcePool
    participant Environment

    User->>CLI: oxo-flow run pipeline.oxoflow -j 4
    CLI->>Config: WorkflowConfig::from_file()
    Config-->>CLI: WorkflowConfig
    CLI->>DAG: WorkflowDag::from_rules()
    DAG-->>CLI: WorkflowDag
    CLI->>DAG: execution_order()
    DAG-->>CLI: Vec<String> (topological order)
    loop For each rule
        CLI->>Executor: execute_rule()
        Executor->>Environment: ensure_environment_ready()
        Environment-->>Executor: environment ready
        Executor->>ResourcePool: check_resources()
        ResourcePool-->>Executor: resources available
        Executor->>ResourcePool: reserve_resources()
        Executor->>Environment: wrap_command()
        Environment-->>Executor: wrapped command
        Executor->>Executor: run shell command
        Executor->>ResourcePool: release_resources()
        Executor-->>CLI: JobRecord
    end
    CLI->>User: Done: N succeeded, M failed
```

---

## Key Design Decisions

### DAG-first execution

All workflows are compiled into a Directed Acyclic Graph before any execution begins. This ensures:

- Dependencies are resolved up front
- Cycles are detected before compute is wasted
- Parallel execution groups are identified
- The execution order is deterministic

### Resource enforcement

Before executing each rule, the executor:

1. **Check**: Validates that required resources (threads, memory) are available in the ResourcePool
2. **Reserve**: Locks resources before starting execution
3. **Execute**: Runs the shell command within resource constraints
4. **Release**: Returns resources to the pool after completion (or on failure/timeout)

This prevents over-subscription of system resources when running multiple jobs concurrently.

### Environment isolation

Every rule can declare its own software environment. The executor:

1. **Resolve**: Maps environment spec to backend (conda, docker, singularity, pixi, venv)
2. **Setup**: Runs setup command on first use (e.g., `conda env create -f env.yaml`)
3. **Cache**: Marks environment as ready to skip setup on subsequent rules
4. **Wrap**: Wraps shell command through environment (e.g., `conda activate ...; <cmd>`)
5. **Execute**: Runs wrapped command

This prevents tool version conflicts between pipeline steps.

### Environment cache persistence

The EnvironmentCache can persist setup state to a JSON file:

- Enables faster startup on subsequent runs
- Skips redundant environment setup
- Shared across workflow runs using the same environments

### Error types

The core library uses `thiserror` for typed errors:

```rust
pub enum OxoFlowError {
    Config(String),
    Dag(String),
    Execution(String),
    Environment { kind: String, message: String },
    ResourceExhausted { rule: String, ... },
    // ...
}
```

The CLI uses `anyhow` for ergonomic error handling at the binary level.

### Async runtime

The executor uses `tokio` for async task execution. Each rule runs as a tokio task, enabling concurrent execution up to the `-j` limit. Resource management uses `Arc<Mutex<ResourcePool>>` for thread-safe access.

### Serialization

All configuration is TOML-based, parsed with `serde` and the `toml` crate. Report output supports both HTML (via Tera templates) and JSON (via serde_json).

---

## Technology Stack

| Component | Technology |
|---|---|
| Language | Rust (edition 2024) |
| Async runtime | tokio |
| CLI framework | clap (derive macros) |
| Web framework | axum |
| Serialization | serde + toml |
| Logging | tracing |
| Error handling | thiserror (lib) + anyhow (bin) |
| Templating | Tera |
| Graph algorithms | petgraph |
| System detection | num_cpus |

---

## See Also

- [DAG Engine](./dag-engine.md) — detailed DAG implementation
- [Environment System](./environment-system.md) — environment resolution architecture
- [Web API](./web-api.md) — REST endpoint design
