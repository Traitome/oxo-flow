# TODO: Expert Panel Evaluation — oxo-flow

> **15 domain experts × 5–7 opinions each = 89 concrete, actionable items**
>
> Focus areas: `.oxoflow` format spec, CLI, web application, DAG engine, and cross-cutting concerns.
>
> Legend: ✅ Resolved | 🔴 Critical | 🟡 Important | 🟢 Nice-to-have
>
> 实现说明 (implementation notes) are provided in Chinese per convention.

---

## Expert 1: Senior Bioinformatics Pipeline Engineer

*Focus: format completeness, wildcard syntax, rule semantics*

1. ✅ 🔴 **Rule `depends_on` field for explicit rule-level dependencies**
   The current `.oxoflow` format only infers dependencies through file-path matching (`input`/`output` overlap). This breaks down for rules with side effects (e.g., database indexing, reference genome prep) where no file is produced that downstream rules consume by name. An explicit `depends_on = ["index_reference"]` field on `Rule` is essential.
   > 实现说明：在 `Rule` 结构体新增 `depends_on: Vec<String>` 字段，`RuleBuilder` 添加链式方法，DAG 构建时将 `depends_on` 边与文件匹配边合并，TOML 序列化/反序列化完整支持。

2. ✅ 🔴 **Rule `retry_delay` for exponential/fixed backoff between retries**
   The `retries` field exists but there is no `retry_delay` — when a network-dependent rule (e.g., downloading from SRA, querying annotation APIs) fails, immediate retry hammers the remote service and almost certainly fails again. A `retry_delay = "30s"` field with human-readable duration syntax is critical for production pipelines.
   > 实现说明：`Rule` 新增 `retry_delay: Option<String>` 字段，支持 "10s"、"2m"、"1h" 格式。`RuleBuilder` 添加 `.retry_delay()` 方法。`format_workflow()` 输出中包含该字段。

3. ✅ 🟡 **Per-rule `workdir` override**
   Many bioinformatics tools (legacy Perl scripts, R packages) assume they run from a specific directory or write to `./` unconditionally. The executor's global `workdir` in `ExecutorConfig` is insufficient — each rule needs its own `workdir = "analysis/{sample}/calling"` to isolate tool-specific directory assumptions.
   > 实现说明：`Rule` 新增 `workdir: Option<String>` 字段，支持通配符展开。`RuleBuilder` 添加 `.workdir()` 方法。执行器在运行规则前切换到指定目录。

4. ✅ 🟡 **Rule `on_success` / `on_failure` hook commands**
   Production pipelines need lifecycle hooks: send Slack notification on failure, upload QC metrics on success, trigger downstream API calls. Currently, the only option is wrapping shell commands in a bash script that does its own error handling — fragile and non-portable.
   > 实现说明：`Rule` 新增 `on_success: Option<String>` 和 `on_failure: Option<String>` 字段，存储 shell 命令字符串。`RuleBuilder` 添加对应方法。执行器在规则完成后根据退出码调用相应 hook。

5. ✅ 🟡 **Lint code W012: `retry` without `retry_delay` is suspicious**
   If a rule sets `retries = 3` but no `retry_delay`, the linter should emit W012 warning the user that immediate retries are rarely useful for transient failures. This catches a common misconfiguration in clinical pipelines where API rate limits cause cascading failures.
   > 实现说明：在 `lint_format()` 中添加 W012 诊断规则，检测 `retries > 0 && retry_delay.is_none()` 条件，生成带建议的 `Diagnostic`。

6. ✅ 🟡 **Lint code W013: `on_failure` without `retries` is a code smell**
   If a rule defines `on_failure` but `retries = 0`, it's likely the author forgot to add retries — or they intentionally want immediate notification. Either way, the linter should flag it as W013 so the intent is explicit.
   > 实现说明：在 `lint_format()` 中添加 W013 诊断规则，检测 `on_failure.is_some() && retries == 0` 条件，建议用户确认是否需要重试。

7. ✅ 🟢 **`diff_workflows()` function to compare two parsed configs**
   When iterating on a pipeline, users need to see what changed between two versions of a `.oxoflow` file: added/removed rules, changed resources, modified shell commands. A structural diff (not textual) that understands rule semantics is far more useful than `diff file1 file2`.
   > 实现说明：在 `format.rs` 中实现 `diff_workflows(a: &WorkflowConfig, b: &WorkflowConfig) -> Vec<WorkflowDiff>` 函数，返回结构化差异列表（规则增删、字段变更、配置变更）。

---

## Expert 2: Clinical Genomics Lab Director

*Focus: patient safety, regulatory compliance, audit trails*

1. ✅ 🔴 **Rule `on_failure` hooks for clinical alerting**
   In a CAP/CLIA-accredited lab, a failed variant calling step MUST trigger an alert to the lab director within minutes — not discovered hours later when someone checks logs. The `on_failure` hook is a patient-safety mechanism, not a convenience feature.
   > 实现说明：`on_failure` hook 支持任意 shell 命令，可调用 `curl` 发送 Slack/PagerDuty 告警，或写入 LIMS 系统。执行器确保 hook 失败不会掩盖原始错误。

2. ✅ 🔴 **Explicit `depends_on` for validation gate rules**
   Clinical pipelines have "validation gate" rules that check QC metrics before proceeding — these produce no files consumed by downstream rules but MUST block execution. File-based dependency inference cannot express "do not call variants until coverage check passes." Explicit `depends_on` is a regulatory requirement.
   > 实现说明：`depends_on` 字段在 DAG 中创建硬依赖边，验证门规则即使无文件输出也能阻断下游执行。`validate_format()` 检查 `depends_on` 引用的规则名是否存在。

3. ✅ 🟡 **`retry_delay` for clinical API integrations**
   Clinical annotation services (ClinVar, OncoKB, CIViC) have strict rate limits. A pipeline that retries immediately after a 429 response will be IP-banned, halting patient reporting for the entire lab. `retry_delay = "60s"` with exponential backoff prevents this catastrophic failure mode.
   > 实现说明：`retry_delay` 字段支持固定延迟和指数退避（executor 可按 delay × 2^attempt 递增）。格式化输出和解析均支持人类可读的时间字符串。

4. ✅ 🟡 **Per-rule `workdir` for containerized clinical tools**
   FDA-cleared tools (e.g., Illumina DRAGEN, Sentieon) often require specific directory structures and write to hardcoded paths. Per-rule `workdir` lets us isolate each tool's filesystem assumptions without modifying validated, locked-down container images.
   > 实现说明：`workdir` 在容器执行时映射为容器内工作目录（`-w` flag for Docker），在本地执行时使用 `std::env::set_current_dir`。

5. ✅ 🟡 **`on_success` hooks for chain-of-custody logging**
   Every completed step in a clinical pipeline should log to an immutable audit trail (WORM storage, blockchain-anchored timestamp). `on_success = "log_chain_of_custody.sh {rule.name} {output}"` enables this without polluting the bioinformatics shell command.
   > 实现说明：`on_success` hook 在规则输出验证通过后执行，支持模板变量 `{rule.name}`。执行器记录 hook 的退出码到 `ExecutionProvenance`。

6. ✅ 🟢 **Workflow diff for validation change control**
   CAP requires documented change control for any pipeline modification. `diff_workflows()` generates the structured diff needed for the change control record — "Rule X: threads changed from 8 to 16, memory unchanged" is what auditors need, not a wall of TOML diff.
   > 实现说明：`diff_workflows()` 输出的 `WorkflowDiff` 包含规则级别的字段变更详情，可序列化为 JSON 用于审计记录系统。

---

## Expert 3: Software Architect

*Focus: architecture patterns, modularity, extensibility*

1. ✅ 🔴 **DAG must support explicit `depends_on` edges alongside inferred edges**
   The current `WorkflowDag::from_rules()` only builds edges from file-path overlap analysis. This conflates two distinct dependency types: data dependencies (file A → file B) and control dependencies (rule X must finish before rule Y starts). The DAG should maintain both edge types for correct scheduling and visualization.
   > 实现说明：`from_rules()` 重构为先构建文件匹配边，再遍历所有规则的 `depends_on` 字段添加控制依赖边。`to_dot()` 可用不同线型区分两种边。

2. ✅ 🔴 **`critical_path()` method on `WorkflowDag`**
   Without critical path analysis, users cannot identify bottleneck rules in a complex pipeline. The DAG has `parallel_groups()` and `metrics()` but no way to answer "which chain of rules determines my total runtime?" This is essential for optimization.
   > 实现说明：在 `WorkflowDag` 实现 `critical_path(&self) -> Result<Vec<String>>`，基于拓扑排序和规则权重（threads × memory 估算）计算最长路径。`DagMetrics` 新增 `critical_path_length` 字段。

3. ✅ 🟡 **`RuleBuilder` must support all new fields with ergonomic chaining**
   The existing `RuleBuilder` pattern is well-designed but must be extended for `depends_on`, `retry_delay`, `workdir`, `on_success`, and `on_failure`. Each builder method should validate its input eagerly (e.g., reject negative retry delays) rather than deferring to `build()`.
   > 实现说明：`RuleBuilder` 新增 `.depends_on()`, `.retry_delay()`, `.workdir()`, `.on_success()`, `.on_failure()` 方法，每个方法内部做基本验证（如时间格式检查）。

4. ✅ 🟡 **`format_workflow()` must serialize new fields correctly**
   The `format_workflow()` function in `format.rs` generates canonical TOML from a `WorkflowConfig`. New fields (`depends_on`, `retry_delay`, `workdir`, `on_success`, `on_failure`) must appear in the output with correct ordering and omit-when-default semantics.
   > 实现说明：`format_workflow()` 中为每个新字段添加条件输出逻辑：`depends_on` 仅在非空时输出为 TOML 数组，`retry_delay` 仅在 `Some` 时输出，`workdir` 同理。字段顺序遵循既有约定。

5. ✅ 🟡 **`diff_workflows()` should return a structured type, not a string**
   A diff function that returns `String` is useless for programmatic consumption (web API, CI integration, change control systems). It should return `Vec<WorkflowDiff>` where `WorkflowDiff` is an enum of `RuleAdded`, `RuleRemoved`, `RuleModified { field, old, new }`, `ConfigChanged`, etc.
   > 实现说明：定义 `WorkflowDiff` 枚举类型和 `WorkflowFieldChange` 结构体。`diff_workflows()` 逐规则比较所有字段，返回类型化的差异列表。支持 `Display` trait 用于人类可读输出。

6. ✅ 🟢 **Validate `depends_on` references at parse time, not just DAG build time**
   If a rule says `depends_on = ["nonexistent_rule"]`, this should fail during `WorkflowConfig::validate()` — not silently pass validation and then fail at DAG construction. Fail-fast principle.
   > 实现说明：`validate_format()` 中添加检查：遍历所有规则的 `depends_on`，确认每个引用的规则名存在于配置中。缺失引用生成 E 级别诊断。

---

## Expert 4: Rust Systems Developer

*Focus: Rust idioms, performance, memory safety, type system usage*

1. ✅ 🔴 **`depends_on` field must use `#[serde(default, skip_serializing_if = "Vec::is_empty")]`**
   Adding `depends_on: Vec<String>` to `Rule` without proper serde attributes will break backward compatibility — existing `.oxoflow` files without the field will fail to parse, and serialization will emit empty arrays unnecessarily. This is a Rust serde footgun that must be handled correctly.
   > 实现说明：所有新的 `Vec` 字段使用 `#[serde(default, skip_serializing_if = "Vec::is_empty")]`，所有新的 `Option` 字段使用 `#[serde(default, skip_serializing_if = "Option::is_none")]`，确保向后兼容。

2. ✅ 🔴 **`retry_delay` should be validated at deserialization, not runtime**
   A `retry_delay = "banana"` should fail at TOML parse time with a clear error message, not panic at runtime when the executor tries to parse it. Implement a custom deserializer or validate in `Rule::validate()`.
   > 实现说明：`Rule::validate()` 中添加 `retry_delay` 格式验证，使用正则匹配 `^\d+[smh]$` 模式。无效格式返回 `OxoFlowError::Validation` 并附带建议。

3. ✅ 🟡 **`critical_path()` must handle zero-weight DAGs gracefully**
   If no rules have resource annotations (threads/memory), the critical path degenerates to the longest chain by node count. The implementation must not divide by zero or produce nonsensical results when all weights are equal.
   > 实现说明：`critical_path()` 在所有权重为零时回退为按节点数计算最长路径。使用 `f64` 权重避免整数溢出，默认权重为 `1.0`。

4. ✅ 🟡 **Builder pattern must return `&mut Self` consistently, not mix `Self` and `&mut Self`**
   The existing `RuleBuilder` uses `&mut self -> &mut Self` pattern. New methods must follow the same convention — inconsistency will break method chaining and confuse users of the API.
   > 实现说明：所有新 `RuleBuilder` 方法统一返回 `&mut Self`，与现有 `.name()`, `.input()`, `.output()` 等方法一致。

5. ✅ 🟡 **`WorkflowDiff` should derive `PartialEq, Debug, Clone, Serialize`**
   Any new public type must derive the standard trait set for testability (`PartialEq` for assertions, `Debug` for error messages, `Serialize` for API responses). Missing derives are a recurring source of friction in Rust codebases.
   > 实现说明：`WorkflowDiff` 和 `WorkflowFieldChange` 派生 `Debug, Clone, PartialEq, Serialize, Deserialize`，与项目中其他公共类型一致。

6. ✅ 🟢 **Test coverage for serde round-trip of all new fields**
   Every new `Rule` field needs a test that serializes to TOML and deserializes back, asserting equality. This catches serde attribute mistakes (wrong default, missing rename) that compile but silently lose data.
   > 实现说明：为每个新字段编写 `#[test]` 函数，构造含新字段的 `Rule`，序列化为 TOML 字符串后反序列化，断言字段值不变。

7. ✅ 🟢 **`diff_workflows()` should be O(n) in rules, not O(n²)**
   A naive implementation that compares every rule in A against every rule in B is O(n²). Use a `HashMap<&str, &Rule>` keyed by rule name for O(n) lookup. Pipelines with 200+ rules exist in production.
   > 实现说明：`diff_workflows()` 内部构建两个 `HashMap<&str, &Rule>` 索引，遍历一次即可检测增删改，总复杂度 O(n)。

---

## Expert 5: DevOps/Infrastructure Engineer

*Focus: deployment, CI/CD, monitoring, operability*

1. ✅ 🔴 **CLI `--base-path` flag on `serve` subcommand**
   Deploying the web interface behind a reverse proxy (nginx, Traefik, AWS ALB) at a sub-path like `/oxo-flow/` is impossible without base path support. All generated URLs, API endpoints, and static asset paths must be prefixed. This is a deployment blocker for any organization with multiple internal tools.
   > 实现说明：`serve` 子命令新增 `--base-path` 参数（默认 "/"）。Web 服务器使用 `base_path` 前缀所有路由，前端 HTML 中的 API URL 动态替换。

2. ✅ 🔴 **Web `/api/metrics` endpoint for system monitoring**
   Production deployments need Prometheus-compatible metrics: request count, latency histograms, active workflow count, queue depth. Without this, the ops team is blind — they can't set up alerts, dashboards, or capacity planning.
   > 实现说明：新增 `/api/metrics` 端点，返回 JSON 格式的系统指标（CPU 使用率、内存、运行中的工作流数量、请求计数、运行时间）。支持 `base_path` 前缀。

3. ✅ 🟡 **CLI `diff` subcommand to compare two `.oxoflow` files**
   In a CI/CD pipeline, PRs that modify a workflow file should show a structural diff in the PR comment — "added rule X, changed memory on rule Y from 8G to 32G." The CLI `diff` subcommand wraps `diff_workflows()` for scripting.
   > 实现说明：CLI 新增 `diff` 子命令，接受两个 `.oxoflow` 文件路径，解析后调用 `diff_workflows()`，输出人类可读的结构化差异。

4. ✅ 🟡 **CLI `touch` subcommand to mark outputs as current**
   After a manual intervention (e.g., fixing a corrupted BAM file by hand), the pipeline needs to know "this output is now valid, don't re-run the rule." The `touch` subcommand updates file timestamps to prevent unnecessary re-execution, similar to `make touch`.
   > 实现说明：CLI 新增 `touch` 子命令，接受工作流文件和规则名列表，使用 `filetime` 更新指定规则输出文件的修改时间为当前时间。

5. ✅ 🟡 **`on_failure` hooks for CI/CD integration**
   In a GitLab CI / GitHub Actions pipeline, a failed workflow step should write a structured JSON failure report that the CI system can parse and annotate. `on_failure = "echo '{\"rule\": \"{rule.name}\", \"exit_code\": $?}' >> failures.jsonl"` enables this pattern.
   > 实现说明：`on_failure` hook 在执行器捕获到非零退出码后触发，支持模板变量替换。hook 本身的失败记录到日志但不改变原始错误状态。

6. ✅ 🟢 **Base path support must handle trailing slash normalization**
   `/oxo-flow` and `/oxo-flow/` must behave identically. A base path of `/oxo-flow/` must not produce double-slash URLs like `/oxo-flow//api/health`. This is a trivial bug that breaks every reverse proxy deployment.
   > 实现说明：`base_path` 在服务器启动时标准化：去除尾部斜杠，确保以 `/` 开头。路由拼接使用 `format!("{}{}", base_path, route)` 避免双斜杠。

---

## Expert 6: UX Designer

*Focus: user experience, CLI ergonomics, web UI usability*

1. ✅ 🔴 **`diff` output must be colored and human-scannable**
   A wall of monochrome text is unusable. Added rules should be green, removed rules red, changed fields yellow. The CLI already has `--no-color` support — the diff subcommand must respect it and default to colored output on TTY.
   > 实现说明：`diff` 子命令输出使用 ANSI 颜色码（绿色=新增、红色=删除、黄色=修改），检测 `--no-color` 标志和 `isatty()` 自动降级。

2. ✅ 🟡 **`touch` subcommand should show what it touched**
   Running `oxo-flow touch workflow.oxoflow --rules bwa_align` should print "Touched 3 files: aligned/sample1.bam, aligned/sample2.bam, aligned/sample3.bam" — not silently succeed. Users need confirmation of what changed.
   > 实现说明：`touch` 执行后输出被更新的文件列表和数量，`--quiet` 模式下仅输出文件路径（便于管道处理）。

3. ✅ 🟡 **Web UI metrics dashboard should auto-refresh**
   A metrics endpoint is useless if users have to manually reload the page. The web UI should poll `/api/metrics` every 5 seconds and update gauges/charts in real-time, similar to the existing SSE-based monitor view.
   > 实现说明：Web 前端 System 页面新增 Metrics 面板，使用 `setInterval` 每 5 秒拉取 `/api/metrics`，动态更新 CPU、内存、工作流计数等指标显示。

4. ✅ 🟡 **Lint warnings W012/W013 should include fix suggestions**
   "W012: Rule 'download_sra' has retries without retry_delay" is informative but not actionable. The diagnostic should include `suggestion: "Add retry_delay = \"30s\" to add a delay between retries"` so users can copy-paste the fix.
   > 实现说明：W012 和 W013 的 `Diagnostic` 包含 `suggestion` 字段，提供可直接复制的 TOML 片段修复建议。

5. ✅ 🟡 **`depends_on` in web editor should offer autocomplete**
   The web editor knows all rule names in the workflow. When typing `depends_on = ["`, the editor should suggest existing rule names — typos in dependency names cause silent failures that are hard to debug.
   > 实现说明：Web 编辑器解析当前工作流中的规则名列表，在 `depends_on` 字段编辑时提供下拉建议。使用已有的 `/api/workflows/parse` 端点获取规则列表。

6. ✅ 🟢 **CLI `diff` should support `--format json` for machine consumption**
   Human-readable diff is great for terminals, but CI systems need structured output. `oxo-flow diff --format json old.oxoflow new.oxoflow` should output the `Vec<WorkflowDiff>` as JSON.
   > 实现说明：`diff` 子命令支持 `--format` 参数（text/json），JSON 模式直接序列化 `diff_workflows()` 返回值。

---

## Expert 7: HPC System Administrator

*Focus: cluster computing, resource management, job scheduling*

1. ✅ 🔴 **`depends_on` must integrate with cluster job dependency syntax**
   When submitting to SLURM, `depends_on` must translate to `--dependency=afterok:<job_id>`. The current cluster submission only chains jobs by file dependencies — explicit control dependencies are lost when moving from local to cluster execution.
   > 实现说明：`ClusterJobConfig` 扩展以携带显式依赖的 job ID 列表。SLURM 提交脚本生成 `#SBATCH --dependency=afterok:${dep_ids}` 行。PBS/SGE/LSF 使用对应语法。

2. ✅ 🔴 **`retry_delay` must be respected by cluster schedulers**
   On a cluster, a failed job that is immediately resubmitted may land on the same faulty node. `retry_delay` should insert a `sleep` or use scheduler-native delay (SLURM `--begin=now+30`). Without this, retries on flaky nodes are an infinite loop.
   > 实现说明：集群提交时将 `retry_delay` 转换为调度器原生延迟参数。SLURM 使用 `--begin=now+{delay}`，PBS 使用 `-a` 时间参数。本地执行使用 `tokio::time::sleep`。

3. ✅ 🟡 **Per-rule `workdir` must be validated as writable on the compute node**
   If `workdir = "/scratch/{sample}"` and `/scratch` is not mounted on the allocated node, the job fails after waiting in queue for hours. The executor should pre-validate `workdir` existence (or create it) before launching the shell command.
   > 实现说明：执行器在规则启动前检查 `workdir` 是否存在，不存在则尝试 `std::fs::create_dir_all`。集群模式下在提交脚本中添加 `mkdir -p` 命令。

4. ✅ 🟡 **`critical_path()` enables cluster budget estimation**
   HPC centers bill by core-hours. `critical_path()` combined with resource annotations lets users estimate minimum wall-clock time and total core-hours BEFORE submitting — avoiding nasty billing surprises on a 10,000-sample cohort.
   > 实现说明：`DagMetrics` 扩展 `critical_path_length` 和 `estimated_core_hours` 字段，`critical_path()` 方法累加路径上各规则的 `threads × time_limit` 估算。

5. ✅ 🟢 **`on_failure` hook should capture SLURM job context**
   When a cluster job fails, the `on_failure` hook should have access to `$SLURM_JOB_ID`, `$SLURM_NODELIST`, etc. — these are essential for debugging node-specific failures (bad GPU, full `/tmp`, network partition).
   > 实现说明：集群执行模式下，`on_failure` hook 在与主命令相同的 SLURM 环境中执行，自动继承所有 `$SLURM_*` 环境变量。

---

## Expert 8: Security Auditor

*Focus: security features, vulnerability prevention, input validation*

1. ✅ 🔴 **`on_success`/`on_failure` hooks are shell injection vectors — must be sanitized**
   If hook commands contain unsanitized user input (e.g., `on_failure = "notify {config.patient_name}"`), a patient name like `; rm -rf /` is catastrophic. Hooks must go through `sanitize_shell_command()` from `executor.rs`.
   > 实现说明：所有 hook 命令在执行前通过 `sanitize_shell_command()` 检查，模板变量替换后再次验证。包含危险字符的命令生成安全警告。

2. ✅ 🔴 **`workdir` must be path-traversal safe**
   A `workdir = "../../etc/cron.d"` is a directory traversal attack. The executor's `validate_path_safety()` must be applied to `workdir` values, rejecting paths that escape the workflow root.
   > 实现说明：`workdir` 在执行前通过 `validate_path_safety()` 验证，拒绝包含 `..`、绝对路径（除非在允许列表中）、符号链接逃逸的路径。

3. ✅ 🟡 **`depends_on` rule names must be validated against injection**
   Rule names are used in shell commands, file paths, and cluster job names. A `depends_on = ["rule; echo pwned"]` must be rejected at parse time. Rule name validation (`[a-zA-Z0-9_-]+`) must apply to `depends_on` references too.
   > 实现说明：`validate_format()` 对 `depends_on` 中的每个名称应用与规则名相同的字符集验证（`^[a-zA-Z][a-zA-Z0-9_-]*$`），无效名称生成 E 级别诊断。

4. ✅ 🟡 **Web `base_path` must not allow path traversal**
   A `--base-path "/../admin"` could route requests to unintended handlers. The base path must be validated: must start with `/`, contain only alphanumeric characters and hyphens, no `..` sequences.
   > 实现说明：`base_path` 在服务器启动时验证，拒绝包含 `..`、非 ASCII 字符、连续斜杠的路径。使用白名单字符集 `[a-zA-Z0-9/_-]`。

5. ✅ 🟡 **`/api/metrics` endpoint should not leak sensitive information**
   System metrics (CPU, memory, disk) can reveal infrastructure details useful for attackers. The metrics endpoint must be behind authentication or at minimum not expose filesystem paths, hostnames, or IP addresses.
   > 实现说明：`/api/metrics` 仅返回聚合数值指标（百分比、计数），不包含路径、主机名、用户名等敏感信息。遵循已有的认证中间件。

6. ✅ 🟢 **`diff_workflows()` output must not leak file contents**
   When diffing workflows, changed `shell` commands may contain credentials or API keys. The diff output should run through `scan_for_secrets()` and redact detected secrets.
   > 实现说明：`diff_workflows()` 对涉及 `shell` 和 `params` 字段变更的差异条目调用 `scan_for_secrets()`，检测到的敏感信息在输出中替换为 `[REDACTED]`。

---

## Expert 9: Scientific Journal Editor

*Focus: reproducibility, provenance tracking, scientific rigor*

1. ✅ 🔴 **`depends_on` makes dependency graphs publication-ready**
   Reviewers increasingly demand DAG visualizations in methods sections. Implicit file-based dependencies produce cluttered, hard-to-interpret graphs. Explicit `depends_on` edges create clean, intentional graphs that convey the actual analysis logic.
   > 实现说明：`to_dot()` 为 `depends_on` 边使用虚线样式（`style=dashed`），与文件依赖实线区分。生成的 DOT 图可直接嵌入论文补充材料。

2. ✅ 🔴 **Workflow diff enables reproducibility auditing**
   When a paper's results cannot be reproduced, the first question is "what changed?" `diff_workflows()` answers this precisely: "version 1.2 added 2 rules, changed GATK version from 4.2 to 4.3, increased memory on variant calling from 16G to 32G."
   > 实现说明：`diff_workflows()` 输出的 `WorkflowDiff` 可直接包含在论文的补充材料中，或嵌入 `ExecutionProvenance` 用于长期存档。

3. ✅ 🟡 **`retry_delay` must be recorded in provenance**
   If a rule was retried 3 times with 30-second delays, the total wall-clock time includes those delays. Provenance records that omit retry information are scientifically misleading — they make the pipeline appear faster than it actually ran.
   > 实现说明：`ExecutionProvenance` 记录每次重试的时间戳和延迟，`BenchmarkRecord` 区分首次执行和重试执行的耗时。

4. ✅ 🟡 **`on_success` hooks should support provenance logging**
   `on_success = "sha256sum {output} >> checksums.txt"` creates a tamper-evident record of every output file. This is essential for journals that require data integrity verification.
   > 实现说明：`on_success` hook 的输出可被 `ExecutionProvenance` 捕获，支持构建完整的输出文件校验链。

5. ✅ 🟢 **`critical_path()` aids methods section writing**
   "The critical path through our pipeline was: alignment (4h) → duplicate marking (1h) → variant calling (6h), totaling 11 hours of sequential computation" — this is exactly what reviewers want to see in the methods section.
   > 实现说明：`critical_path()` 返回值包含每个节点的估算时间，可格式化为论文方法部分所需的自然语言描述。

---

## Expert 10: Bioinformatics Graduate Student

*Focus: beginner usability, learning curve, documentation quality*

1. ✅ 🔴 **`depends_on` is more intuitive than file-path matching for beginners**
   New users spend hours debugging "why doesn't my rule run?" because they don't understand implicit file-based dependency inference. `depends_on = ["align"]` is immediately obvious — "this rule runs after alignment finishes."
   > 实现说明：`depends_on` 提供显式、易理解的依赖声明，降低新用户的学习曲线。文档中优先使用 `depends_on` 示例。

2. ✅ 🔴 **Error messages for `depends_on` typos must be helpful**
   If a student writes `depends_on = ["bwa_algin"]` (typo), the error must say "Rule 'bwa_algin' not found. Did you mean 'bwa_align'?" — not a cryptic DAG construction failure. Fuzzy matching on rule names is essential.
   > 实现说明：`validate_format()` 中对 `depends_on` 引用的未知规则名使用编辑距离算法（Levenshtein）查找最相似的现有规则名，包含在 `suggestion` 字段中。

3. ✅ 🟡 **`diff` subcommand helps students learn from changes**
   When a supervisor modifies a student's workflow, `oxo-flow diff original.oxoflow modified.oxoflow` shows exactly what was changed and why (if descriptions are updated). This is a powerful teaching tool.
   > 实现说明：`diff` 输出包含规则描述的变更，帮助新用户理解每次修改的意图。`--verbose` 模式显示完整的字段级对比。

4. ✅ 🟡 **`touch` prevents frustrating re-runs during development**
   Students iterating on the last step of a pipeline don't want to re-run the entire 8-hour alignment just because they modified the variant calling rule. `touch` marks upstream outputs as current, enabling fast iteration.
   > 实现说明：`touch` 命令的帮助文本包含常见使用场景示例，`--dry-run` 模式显示将要更新的文件列表但不实际修改。

5. ✅ 🟡 **W012/W013 lint warnings teach best practices**
   Lint warnings aren't just error prevention — they're educational. "W012: Consider adding retry_delay when using retries" teaches students about transient failure handling patterns they wouldn't learn from a textbook.
   > 实现说明：W012 和 W013 的诊断消息包含简短解释（"Retrying immediately after failure rarely helps for network or API errors"），不仅指出问题还解释原因。

6. ✅ 🟢 **Web UI should show `depends_on` edges in DAG visualization**
   The web UI's DAG view is the most visual way to understand a pipeline. `depends_on` edges should be displayed with a distinct style (dashed lines) and labeled "explicit dependency" on hover.
   > 实现说明：Web 前端 DAG 视图解析 `to_dot()` 输出中的虚线边，渲染为可视化区分的依赖线条，hover 显示依赖类型。

---

## Expert 11: Regulatory Affairs Specialist (IVD)

*Focus: IVD compliance, 21 CFR Part 11, EU IVDR, validation protocols*

1. ✅ 🔴 **`on_failure` hooks are required for 21 CFR Part 11 compliance**
   FDA 21 CFR Part 11 §11.10(a) requires "validation of systems to ensure accuracy, reliability, consistent intended performance." A pipeline that fails silently — without triggering a documented alert — violates this requirement. `on_failure` hooks enable compliant alerting.
   > 实现说明：`on_failure` hook 提供可审计的失败响应机制，hook 执行记录写入 `ExecutionProvenance`，满足 FDA 电子记录要求。

2. ✅ 🔴 **`depends_on` enforces validated execution order**
   IVD software validation requires demonstrating that steps execute in the validated order. File-based inference is fragile — renaming an output file could silently change execution order. `depends_on` creates an explicit, auditable execution contract.
   > 实现说明：`depends_on` 边在 DAG 验证时强制检查，任何违反显式依赖顺序的情况生成 `CycleDetected` 错误。验证报告包含依赖图的完整性证明。

3. ✅ 🟡 **Workflow diff is essential for IVD change control (EU IVDR Article 82)**
   EU IVDR requires documented change control for any modification to IVD software. `diff_workflows()` generates the structured change record needed for the technical file — auditors need field-level changes, not git diffs.
   > 实现说明：`diff_workflows()` 输出的 JSON 格式可直接嵌入 IVD 技术文件的变更控制记录部分，包含版本号、变更日期、字段级差异。

4. ✅ 🟡 **`retry_delay` must be documented in the validation protocol**
   If a validated pipeline uses `retries = 3, retry_delay = "30s"`, the validation protocol must specify these exact values. Any change requires re-validation. The `format_workflow()` canonical output serves as the validated configuration reference.
   > 实现说明：`format_workflow()` 的确定性输出确保相同配置始终生成相同的 TOML 文本，可用作验证协议的规范参考文档。

5. ✅ 🟡 **`/api/metrics` supports continuous monitoring per IEC 62304**
   IEC 62304 (medical device software lifecycle) requires post-market surveillance including performance monitoring. The `/api/metrics` endpoint enables automated monitoring dashboards that satisfy this requirement.
   > 实现说明：`/api/metrics` 端点提供的运行时指标支持持续性能监控，可集成到医疗器械后市场监控系统中。

---

## Expert 12: Full-Stack Web Developer

*Focus: web application quality, API design, frontend experience*

1. ✅ 🔴 **Base path support is essential for enterprise deployment**
   No enterprise deploys a web app at the root URL. Internal tools live at `/tools/oxo-flow/`, behind OAuth proxies at `/auth/oxo-flow/`, or in Kubernetes ingress at `/ns/bioinformatics/oxo-flow/`. Without base path support, the web UI is demo-only.
   > 实现说明：Axum 路由器使用 `nest()` 方法将所有路由嵌套在 `base_path` 下。前端 HTML 中的 API 基础 URL 通过模板变量 `{{base_path}}` 注入。

2. ✅ 🔴 **`/api/metrics` must return structured JSON, not Prometheus text format**
   The existing API is JSON-only. Adding a Prometheus text endpoint is inconsistent. Return JSON with fields like `{ "cpu_usage_percent": 45.2, "memory_used_bytes": 1073741824, "active_workflows": 3, "uptime_seconds": 86400 }`.
   > 实现说明：`/api/metrics` 返回 JSON 对象，包含 `cpu_usage_percent`、`memory_used_bytes`、`active_workflows`、`total_requests`、`uptime_seconds` 等字段。

3. ✅ 🟡 **Web diff endpoint: `POST /api/workflows/diff`**
   The web UI should expose workflow diffing via API. Accept `{ "workflow_a": "...", "workflow_b": "..." }` and return `Vec<WorkflowDiff>` as JSON. This enables web-based change review without CLI access.
   > 实现说明：新增 `POST /api/workflows/diff` 端点，接受两个 TOML 字符串，解析后调用 `diff_workflows()`，返回 JSON 格式的差异列表。

4. ✅ 🟡 **All new endpoints must respect `base_path` prefix**
   If `base_path = "/oxo-flow"`, then `/api/metrics` becomes `/oxo-flow/api/metrics`. This must be tested — base path bugs are silent until deployment and then break everything at once.
   > 实现说明：所有新路由通过 `nest()` 注册，自动继承 `base_path` 前缀。集成测试验证 base_path 场景下的路由可达性。

5. ✅ 🟡 **Frontend must handle `base_path` in all fetch() calls**
   A hardcoded `fetch('/api/health')` breaks when deployed at `/oxo-flow/`. All frontend API calls must use `fetch(\`${BASE_PATH}/api/health\`)` where `BASE_PATH` is injected at page load.
   > 实现说明：前端 JavaScript 从页面 `<meta>` 标签或全局变量读取 `BASE_PATH`，所有 `fetch()` 调用使用模板字符串拼接基础路径。

6. ✅ 🟢 **Metrics endpoint should include request latency percentiles**
   p50, p95, p99 latency percentiles are essential for SLA monitoring. The current health endpoint only returns "ok" — metrics should show actual performance characteristics.
   > 实现说明：`AppState` 扩展请求计数器和延迟直方图，`/api/metrics` 输出包含 `request_latency_p50_ms`、`request_latency_p95_ms` 等百分位指标。

---

## Expert 13: System Administrator

*Focus: deployment, maintenance, operational stability*

1. ✅ 🔴 **`serve --base-path` must be documented with nginx/Traefik examples**
   The feature is useless without deployment examples. Show the exact nginx `location /oxo-flow/ { proxy_pass http://127.0.0.1:8080/oxo-flow/; }` config — sysadmins will not guess the correct proxy configuration.
   > 实现说明：CLI `--help` 文本和文档包含 nginx 和 Traefik 反向代理配置示例，展示 `base_path` 与代理路径的对应关系。

2. ✅ 🔴 **Metrics endpoint enables Nagios/Zabbix monitoring**
   Not every org uses Prometheus. The JSON metrics endpoint is universally parseable by any monitoring system. Ensure it includes `uptime_seconds` and `version` for basic health monitoring.
   > 实现说明：`/api/metrics` 始终包含 `version` 和 `uptime_seconds` 字段，兼容任何能解析 JSON 的监控系统（Nagios check_http、Zabbix HTTP agent 等）。

3. ✅ 🟡 **`on_failure` hooks enable self-healing and escalation**
   `on_failure = "systemctl restart oxo-flow-worker"` or `on_failure = "mail -s 'Pipeline failed' admin@org.com"` — sysadmins need these hooks to build operational runbooks without wrapping every command in custom monitoring scripts.
   > 实现说明：`on_failure` hook 以与主命令相同的用户权限执行，支持任意 shell 命令，可调用系统工具进行告警和自动恢复。

4. ✅ 🟡 **`touch` subcommand prevents unnecessary compute waste**
   When a storage migration changes file timestamps, the entire pipeline would re-run. `oxo-flow touch workflow.oxoflow --all` resets all output timestamps, preventing terabytes of unnecessary recomputation.
   > 实现说明：`touch` 支持 `--all` 标志更新工作流中所有规则的输出文件时间戳，`--rules` 标志限定特定规则。

5. ✅ 🟢 **Base path should be configurable via environment variable too**
   In containerized deployments (Docker, Kubernetes), CLI flags are less convenient than environment variables. `OXO_FLOW_BASE_PATH=/oxo-flow` should work alongside `--base-path`.
   > 实现说明：`serve` 子命令通过 `clap` 的 `env = "OXO_FLOW_BASE_PATH"` 属性同时支持命令行参数和环境变量配置。

---

## Expert 14: Data Scientist

*Focus: data integration, analysis workflows, notebook interop*

1. ✅ 🔴 **`depends_on` enables mixed compute/analysis workflows**
   Data science workflows often have steps that don't produce files consumed by the next step — e.g., "train model" → "evaluate model" → "generate report" where the model is in a database, not a file. `depends_on` makes oxo-flow viable for ML pipelines.
   > 实现说明：`depends_on` 支持无文件依赖的规则链，扩展 oxo-flow 从纯生物信息学工具到通用数据科学工作流引擎。

2. ✅ 🟡 **`retry_delay` for API-heavy data science workflows**
   Data science pipelines hit external APIs constantly: model registries, feature stores, data catalogs, annotation services. Rate limiting is a fact of life — `retry_delay` prevents the entire pipeline from collapsing when one API returns 429.
   > 实现说明：`retry_delay` 在 API 调用密集的场景中尤为重要，支持固定和指数退避策略，防止级联失败。

3. ✅ 🟡 **`diff` subcommand for experiment tracking**
   Data scientists iterate rapidly: "I changed the filtering threshold from 0.05 to 0.01, added a new normalization step, and increased memory." `oxo-flow diff v1.oxoflow v2.oxoflow` creates the experiment changelog that MLflow/W&B can't capture.
   > 实现说明：`diff` 输出的结构化格式可集成到实验追踪系统（MLflow、Weights & Biases），通过 `--format json` 支持程序化记录。

4. ✅ 🟡 **`on_success` hooks for metric logging**
   `on_success = "python log_metrics.py --rule {rule.name} --output {output}"` lets data scientists log QC metrics, model performance, or data quality scores to their tracking system without modifying the analysis script.
   > 实现说明：`on_success` hook 在规则成功后执行，支持调用 Python 脚本进行指标记录，模板变量提供规则上下文信息。

5. ✅ 🟢 **`critical_path()` for compute cost estimation**
   Cloud compute is billed by the minute. Before running a pipeline on AWS/GCP, data scientists need to estimate cost. `critical_path()` × instance pricing gives a minimum cost estimate.
   > 实现说明：`critical_path()` 结合 `resources` 注解计算路径权重，可用于估算最小计算成本（关键路径时间 × 资源单价）。

6. ✅ 🟢 **Per-rule `workdir` for R/Python scripts with relative paths**
   R's `setwd()` and Python's `os.chdir()` are dangerous when called inside a pipeline. Per-rule `workdir` handles this at the orchestration level, so scripts don't need to know where they run.
   > 实现说明：`workdir` 在执行器层面设置进程工作目录，脚本内部无需调用 `setwd()` 或 `os.chdir()`，避免路径管理冲突。

---

## Expert 15: Open Source Community Manager

*Focus: community growth, ecosystem health, contributor experience*

1. ✅ 🔴 **`depends_on` is the #1 feature request in every workflow engine**
   Snakemake, Nextflow, WDL — every pipeline tool's issue tracker has hundreds of requests for explicit dependencies beyond file matching. Implementing this positions oxo-flow as the first Rust-based workflow engine to get it right natively.
   > 实现说明：`depends_on` 的实现参考了 Snakemake 的 `rule` 依赖和 Nextflow 的 `channel` 模式，但在 TOML 格式中更加声明式和直观。

2. ✅ 🟡 **`diff` subcommand enables better PR reviews**
   Open source contributors modify `.oxoflow` files in PRs. Reviewers need to understand the impact of changes without parsing TOML mentally. `oxo-flow diff` in CI generates clear summaries that speed up code review for pipeline changes.
   > 实现说明：`diff` 可在 GitHub Actions 中运行并将结构化差异作为 PR 评论发布，提高开源项目的协作效率。

3. ✅ 🟡 **New lint codes demonstrate active development**
   Users evaluate tool maturity by linter sophistication. W012 and W013 show that oxo-flow's linter understands pipeline semantics, not just syntax — this differentiates it from "just another TOML validator."
   > 实现说明：W012 和 W013 展示了 oxo-flow 对工作流语义的深度理解，提升社区对项目成熟度的信心。

4. ✅ 🟡 **Web UI base path support enables hosted demo instances**
   An open source project needs a live demo. With base path support, the oxo-flow team can deploy a demo at `https://oxo-flow.dev/demo/` alongside the documentation — lowering the barrier to trying the tool.
   > 实现说明：`base_path` 支持使得在共享域名下部署多个 oxo-flow 实例成为可能，适用于演示、培训和多租户场景。

5. ✅ 🟡 **Comprehensive test coverage builds contributor confidence**
   New contributors won't submit PRs to a project with poor test coverage — they can't verify their changes don't break things. The test suite for new features (builder, diff, lint, DAG) signals "this project takes quality seriously."
   > 实现说明：所有新功能包含完整的单元测试和集成测试，测试覆盖率报告在 CI 中自动生成，PR 模板要求描述测试策略。

6. ✅ 🟢 **`on_success`/`on_failure` hooks enable ecosystem integrations**
   Hooks are the extension point that enables an ecosystem: Slack bots, Jira integrations, custom dashboards, lab LIMS connections. Without hooks, every integration requires forking the executor.
   > 实现说明：hook 机制提供了轻量级的扩展点，社区可以构建和分享 hook 脚本集合（如 `oxo-flow-hooks` 仓库），形成插件生态。

7. ✅ 🟢 **`/api/metrics` enables community monitoring dashboards**
   Community members will build Grafana dashboards, monitoring scripts, and status pages around the metrics endpoint. Documenting the JSON schema enables this ecosystem without core team effort.
   > 实现说明：`/api/metrics` 的 JSON 响应格式在 API 文档中完整描述，包含字段类型、单位和示例值，便于社区构建监控集成。

---

## Summary Statistics

| Category | Count |
|----------|-------|
| Total opinions | 89 |
| 🔴 Critical | 27 |
| 🟡 Important | 45 |
| 🟢 Nice-to-have | 17 |
| Experts consulted | 15 |
| Unique features covered | 12 |

### Feature Cross-Reference Matrix

| Feature | Experts who flagged it |
|---------|----------------------|
| `depends_on` field | 1, 2, 3, 7, 8, 9, 10, 11, 14, 15 (10/15) |
| `retry_delay` field | 1, 2, 4, 7, 9, 11, 14 (7/15) |
| `workdir` override | 1, 2, 7, 8, 14 (5/15) |
| `on_success`/`on_failure` hooks | 1, 2, 5, 7, 8, 9, 13, 14, 15 (9/15) |
| `critical_path()` method | 3, 4, 7, 9, 14 (5/15) |
| CLI `diff` subcommand | 5, 6, 10, 14, 15 (5/15) |
| CLI `touch` subcommand | 5, 6, 10, 13 (4/15) |
| Web `base_path` support | 5, 6, 8, 12, 13, 15 (6/15) |
| Web metrics endpoint | 5, 8, 11, 12, 13, 15 (6/15) |
| Lint W012/W013 | 1, 6, 10, 15 (4/15) |
| `diff_workflows()` function | 1, 2, 3, 4, 8, 9, 11 (7/15) |
| `format_workflow()` updates | 3, 4, 11 (3/15) |
| `RuleBuilder` updates | 3, 4 (2/15) |
| Test coverage | 4, 15 (2/15) |
