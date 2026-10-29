//! CLI override parsing/validation: --arg KEY=VALUE handling, key canonicalization, unknown-key warnings, run-flag registries, retired-key diagnostics (#948 P2-5).

use super::*;

/// Long-form names of `oxo-flow run` flags (plus clap's global --help/
/// --version). Keep in sync with the `Run` variant in main.rs.
///
/// clap's trailing `config_overrides` positional (`allow_hyphen_values`)
/// cannot distinguish `--json` the run flag from `--json` a hyphen-value
/// once positional overrides start, so flags typed after `KEY=VALUE` land
/// in the override list (issue #71). Matching them here turns a confusing
/// "invalid config flag" error — or a silently wrong override — into
/// actionable guidance.
pub(crate) const RUN_FLAG_NAMES: &[&str] = &[
    "jobs",
    "keep-going",
    "workdir",
    "target",
    "module",
    "retry",
    "timeout",
    "log-file",
    "resume-failed",
    "profile",
    "max-threads",
    "max-memory",
    "max-submitted",
    "skip-env-setup",
    "skip-ref-build",
    "cache-dir",
    "provenance",
    "bundle",
    "yes",
    "arg",
    "ai-recover",
    "ai-max-retries",
    "samples",
    "rerun",
    "no-report-snapshot",
    "background",
    "json",
    "quiet",
    "verbose",
    "no-color",
    "help",
    "version",
];

/// Short-form run flag names (same contract as [`RUN_FLAG_NAMES`]).
pub(crate) const RUN_SHORT_FLAGS: &[&str] = &["j", "k", "d", "t", "r", "h", "V"];

/// Run flags that `dry-run` also accepts as real command flags (issue
/// #441: `--cache-dir` was added for run→dry-run transcription parity).
/// When the swallowed-flag parser runs under `dry-run`, these names read
/// as a misordered command flag, not "not supported".
pub(crate) const DRY_RUN_SHARED_FLAGS: &[&str] = &["cache-dir"];

/// Flags removed from `run`, mapped to their replacement.
///
/// They must NOT live in [`RUN_FLAG_NAMES`]: that list names real command
/// flags, and a workflow declaring a config key with one of these names
/// (`sample = {...}`) would have its `--sample VALUE` override rejected as a
/// command flag (audit finding). Reaching the unknown-argument path instead
/// still gets a migration hint.
pub(crate) const REMOVED_RUN_FLAGS: &[(&str, &str)] = &[("sample", "--samples")];

/// Validate a config value against its ConfigDef declaration.
///
/// `from_cli` distinguishes an EXPLICIT user-provided value from a
/// workflow-declared default: an empty string from the CLI is a legitimate
/// derive-if-empty override (issue #430) and skips the path checks, while
/// defaults stay strict.
pub(crate) fn validate_config_value(
    name: &str,
    value: &str,
    def: &oxo_flow_core::config::ConfigDef,
    from_cli: bool,
) -> anyhow::Result<()> {
    // ── choices ──
    if let Some(ref choices) = def.choices
        && !choices.iter().any(|c| c == value)
    {
        anyhow::bail!(
            "invalid value '{}' for config '{}'. Allowed: [{}]\n  {}",
            value,
            name,
            choices.join(", "),
            def.help.as_deref().unwrap_or("")
        );
    }

    // ── type validation ──
    if let Some(ref type_) = def.type_ {
        match type_.as_str() {
            "int" => {
                value.parse::<i64>().map_err(|_| {
                    anyhow::anyhow!(
                        "config '{}' expects an integer, got '{}'\n  {}",
                        name,
                        value,
                        def.help.as_deref().unwrap_or("")
                    )
                })?;
            }
            "float" => {
                value.parse::<f64>().map_err(|_| {
                    anyhow::anyhow!(
                        "config '{}' expects a float, got '{}'\n  {}",
                        name,
                        value,
                        def.help.as_deref().unwrap_or("")
                    )
                })?;
            }
            "bool" => {
                if value != "true" && value != "false" {
                    anyhow::bail!(
                        "config '{}' expects a boolean (true/false), got '{}'\n  {}",
                        name,
                        value,
                        def.help.as_deref().unwrap_or("")
                    );
                }
            }
            "path" => {
                // issue #430: an explicitly-passed EMPTY value is a
                // legitimate override — pipelines branch on
                // `[ -n "{config.x}" ]` for derive-if-empty behavior, so
                // `--gene_bed ""` selects "derive from fasta+gtf" and must
                // bypass both the empty-path check and `must_exist` (there
                // is no file to exist yet). Only DEFAULTS keep the strict
                // check: a workflow author's default must point at a real
                // file.
                if value.is_empty() && from_cli {
                    return Ok(());
                }
                if value.is_empty() {
                    anyhow::bail!(
                        "config '{}' expects a path, got empty string\n  {}",
                        name,
                        def.help.as_deref().unwrap_or("")
                    );
                }
                // must_exist check
                if def.must_exist {
                    let p = std::path::Path::new(value);
                    if !p.exists() {
                        anyhow::bail!(
                            "config '{}' path does not exist: '{}'\n  {}",
                            name,
                            value,
                            def.help.as_deref().unwrap_or("")
                        );
                    }
                }
            }
            _ => { /* "string" or unknown — no validation */ }
        }
    }

    // ── range validation (requires type = int or float) ──
    if let Some(ref range) = def.range
        && let Some((min_str, max_str)) = range.split_once("..")
    {
        let min: f64 = min_str.trim().parse().map_err(|_| {
            anyhow::anyhow!("invalid range min '{}' for config '{}'", min_str, name)
        })?;
        let max: f64 = max_str.trim().parse().map_err(|_| {
            anyhow::anyhow!("invalid range max '{}' for config '{}'", max_str, name)
        })?;
        let val: f64 = value.parse().map_err(|_| {
            anyhow::anyhow!(
                "config '{}' has range '{range}' but value '{}' is not numeric\n  {}",
                name,
                value,
                def.help.as_deref().unwrap_or("")
            )
        })?;
        if val < min || val > max {
            anyhow::bail!(
                "config '{}' value {} is outside range {}..{}\n  {}",
                name,
                val,
                min,
                max,
                def.help.as_deref().unwrap_or("")
            );
        }
    }

    Ok(())
}

/// Attempt count for a single-shot AI operation in `run`/`dry-run`.
///
/// `--ai-max-retries N` bounds how many times a FAILED AI call is attempted
/// (1 when the flag is absent — the pre-flag behavior); a successful call is
/// never repeated. The workflow's `[ai] max_retries` is not consulted here:
/// that budget drives the agentic orchestrator used by `template`, which
/// neither the recovery diagnosis nor the dry-run analysis goes through
/// (audit finding: the flag was accepted and discarded).
pub(crate) fn ai_attempts(cli_max_retries: Option<u32>) -> u32 {
    cli_max_retries.unwrap_or(1).max(1)
}

/// Pull the real exit code out of a failure reason ("exit code 137 — OOM …").
/// Panics and aborts have no code in the reason and keep -1.
pub(crate) fn failure_exit_code(failure_reason: &str) -> i32 {
    failure_reason
        .lines()
        .next()
        .and_then(|line| line.split("exit code ").nth(1))
        .and_then(|rest| {
            rest.split_whitespace()
                .next()
                .and_then(|tok| tok.parse().ok())
        })
        .unwrap_or(-1)
}

#[allow(clippy::too_many_arguments)]
/// Parse CLI config overrides — the SAME accepted forms for `run` and
/// `dry-run` (issue #77 parity):
///   KEY=VALUE            direct positional form
///   --KEY=VALUE          long-flag form
///   --KEY VALUE          long-flag form with separate value (only for
///                        config keys the workflow DECLARES — an unknown
///                        `--token` is a typo'd command flag and must not
///                        be silently swallowed as an override, issue #71)
///   --arg KEY=VALUE      legacy `--arg` form (backward compatible)
///
/// Override keys are canonicalized onto their declared `[config]` spelling:
/// `--api-token=x` names the declared key `api_token` (hyphen/underscore
/// variants resolve when the normalization is unambiguous, issue #794).
/// This matters most for `sensitive` keys — validation, masking, and the
/// checkpoint snapshot are keyed on the declared spelling — but it applies
/// to every declared key. Keys that match nothing declared are untouched:
/// `KEY=VALUE` keeps its issue #62 injection semantics, and an unknown
/// `--` token is rejected (issue #71).
///
/// `command` names the invoking subcommand ("run", "dry-run", …) so a
/// swallowed run flag under a command that does not support it says so
/// (issue #430): under `dry-run`, `-j` reads "not supported by dry-run",
/// not the run-ordering hint.
pub(crate) fn parse_cli_overrides(
    cli_args: Vec<String>,
    declared_config_keys: &std::collections::HashSet<String>,
    command: &str,
) -> anyhow::Result<std::collections::HashMap<String, String>> {
    let mut cli_arg_values: std::collections::HashMap<String, String> =
        std::collections::HashMap::new();
    let mut cli_args_iter = cli_args.into_iter().peekable();
    while let Some(arg_str) = cli_args_iter.next() {
        // issue #71: run flags swallowed by the trailing overrides positional
        // get a targeted error instead of a confusing parse failure.
        let swallowed_flag = arg_str
            .strip_prefix("--")
            .map(|flag| flag.split('=').next().unwrap_or(flag))
            .filter(|name| RUN_FLAG_NAMES.contains(name))
            .map(|name| format!("--{name}"))
            .or_else(|| {
                arg_str
                    .strip_prefix('-')
                    .filter(|rest| !rest.starts_with('-'))
                    .map(|flag| flag.split('=').next().unwrap_or(flag))
                    .filter(|name| RUN_SHORT_FLAGS.contains(name))
                    .map(|name| format!("-{name}"))
            });
        if let Some(flag) = swallowed_flag {
            // issue #430: dry-run (and every other command sharing this
            // parser) accepts only a subset of run's flags — a token like
            // `-j` under `dry-run` is not a misordered flag but one the
            // command does not support at all, so the message is
            // command-aware.
            let flag_name = flag.trim_start_matches('-');
            let is_run_flag = RUN_FLAG_NAMES.contains(&flag_name)
                || (flag_name.len() == 2 && RUN_SHORT_FLAGS.contains(&&flag_name[1..]));
            let hint = if command == "run" {
                format!(
                    "'{flag}' is a command flag, not a config override.\n  \
                     Command flags must come before KEY=VALUE overrides, e.g.:\n  \
                     oxo-flow run <workflow.oxoflow> --json min_quality=30\n  \
                     For a config key that itself starts with dashes, use --arg KEY=VALUE\n  \
                     (also placed before positional overrides)."
                )
            } else if is_run_flag {
                // Issue #441: dry-run shares some run flags for real
                // (`--cache-dir`) — for those, a swallowed token is a
                // misordered command flag, not an unsupported one.
                let shared = command == "dry-run" && DRY_RUN_SHARED_FLAGS.contains(&flag_name);
                if shared {
                    format!(
                        "'{flag}' is a command flag, not a config override.\n  \
                         Command flags must come before KEY=VALUE overrides, e.g.:\n  \
                         oxo-flow {command} <workflow.oxoflow> --cache-dir .env-cache min_quality=30"
                    )
                } else {
                    format!(
                        "'{flag}' is a run flag and is not supported by {command}.\n  \
                         Run flags only work on `oxo-flow run`; {command} accepts its own \
                         flags before KEY=VALUE overrides, e.g.:\n  \
                         oxo-flow {command} <workflow.oxoflow> --target my_rule min_quality=30"
                    )
                }
            } else {
                format!(
                    "'{flag}' is not supported by {command} and is not a config override.\n  \
                     Config overrides take KEY=VALUE, e.g.:\n  \
                     oxo-flow {command} <workflow.oxoflow> min_quality=30"
                )
            };
            anyhow::bail!("{hint}");
        }
        let (mut k, v) = if let Some(eq) = arg_str.find('=') {
            let k = arg_str[..eq].trim_start_matches('-').to_string();
            (k, arg_str[eq + 1..].to_string())
        } else if let Some(k) = arg_str.strip_prefix("--") {
            // Space form: the declared check accepts hyphen-spelled keys that
            // canonicalize onto a declared key (`--api-token VALUE` names
            // `api_token`, issue #794). Unknown tokens still get the issue
            // #71 rejection — canonicalization never widens that gate.
            let space_form_key = if declared_config_keys.contains(k) {
                Some(k.to_string())
            } else {
                canonicalize_override_key(k, declared_config_keys)
            };
            if let Some(k) = space_form_key {
                // `--KEY VALUE` — consume the next argument as the value.
                // A following token that starts with `--` is a command flag
                // the user meant to pass, never a config value: swallowing
                // it silently disabled the flag (`dry-run wf --mode --json`
                // ran without JSON output, audit finding). The `=` form
                // already errors on the unknown token, so point there.
                if let Some(next) = cli_args_iter.peek()
                    && next.starts_with("--")
                {
                    anyhow::bail!(
                        "config flag '--{k}' expects a value, but the next argument '{next}' \
                         is a command flag.\n  Use the '=' form (--{k}=<value>) or place the \
                         override after the command flags as {k}=<value>."
                    );
                }
                let v = cli_args_iter.next().with_context(|| {
                    format!(
                        "invalid config flag: '{arg_str}' — expected --KEY=VALUE or --KEY VALUE"
                    )
                })?;
                (k.to_string(), v)
            } else {
                // issue #71 follow-up: a `--` token that is neither a
                // registered flag (caught above) nor a declared config key
                // is almost certainly a typo'd command flag (e.g.
                // `--config x`). Never silently swallow it as an override.
                let rename = REMOVED_RUN_FLAGS
                    .iter()
                    .find(|(removed, _)| *removed == k)
                    .map(|(removed, replacement)| {
                        format!("\n  '--{removed}' was renamed to {replacement}.")
                    })
                    .unwrap_or_default();
                anyhow::bail!(
                    "unknown argument '{arg_str}' — did you mean KEY=VALUE overrides?{rename}\n  \
                     Config overrides take KEY=VALUE (e.g. threads=8) or --KEY=VALUE, e.g.:\n  \
                     oxo-flow {command} <workflow.oxoflow> threads=8; \
                     for a config key that itself starts with dashes, use --arg KEY=VALUE"
                );
            }
        } else {
            anyhow::bail!(
                "invalid config value format: '{arg_str}' — expected KEY=VALUE, --KEY=VALUE, or --KEY VALUE"
            );
        };
        if k.is_empty() {
            anyhow::bail!("invalid config value format: '{arg_str}' — KEY must be non-empty");
        }
        // issue #794: canonicalize onto the declared spelling so a hyphen-
        // spelled override (`--api-token=x` for declared `api_token`) lands
        // on the key the workflow declared — inheriting its validation,
        // sensitive masking, and space-form eligibility. Without this the
        // override is stored under the dashed alias, the declared key keeps
        // its default, and the value leaks into the checkpoint unmasked.
        if !declared_config_keys.contains(&k)
            && let Some(canonical) = canonicalize_override_key(&k, declared_config_keys)
        {
            k = canonical;
        }
        // issue #430: an EMPTY VALUE is a legitimate override — pipelines
        // branch on `[ -n "{config.x}" ]` for derive-if-empty behavior
        // (rnaseq gene_bed/chrom_sizes/transcript_fasta), and the CLI must
        // be able to select that branch (`gene_bed=`, `--gene_bed ""`).
        // Only an empty KEY is a syntax error.
        cli_arg_values.insert(k, v);
    }
    Ok(cli_arg_values)
}

/// Resolve an override key that does not exactly match a declared `[config]`
/// key by normalizing hyphens to underscores (issue #794). Returns the
/// declared key only when the normalization is unambiguous — a normalized
/// form that would map to two declared keys is left alone (the caller then
/// applies issue #62/#71 semantics to the literal spelling). Callers invoke
/// this only after an exact-match check, and a key reaching here always
/// contains a hyphen, so the result never equals the input.
pub(crate) fn canonicalize_override_key(
    key: &str,
    declared_config_keys: &std::collections::HashSet<String>,
) -> Option<String> {
    if !key.contains('-') {
        return None;
    }
    let normalized = key.replace('-', "_");
    if declared_config_keys.contains(&normalized) {
        return Some(normalized);
    }
    let matches: Vec<&String> = declared_config_keys
        .iter()
        .filter(|d| d.replace('-', "_") == normalized)
        .collect();
    match matches.as_slice() {
        [only] => Some((*only).clone()),
        _ => None,
    }
}

/// Apply parsed overrides and the defaults of declarative config entries
/// (`key = { default, required, … }` in `[config]`). Shared by `run` and
/// `dry-run` so preview and execution validate identically.
pub(crate) fn apply_cli_overrides(
    config: &mut WorkflowConfig,
    cli_arg_values: &std::collections::HashMap<String, String>,
) -> anyhow::Result<()> {
    for (name, cfg_def) in &config.config_meta {
        let effective_value = if let Some(val) = cli_arg_values.get(name) {
            validate_config_value(name, val, cfg_def, true)?;
            config
                .config
                .insert(name.clone(), toml::Value::String(val.clone()));
            val.clone()
        } else if let Some(ref default) = cfg_def.default {
            validate_config_value(name, default, cfg_def, false)?;
            config
                .config
                .entry(name.clone())
                .or_insert_with(|| toml::Value::String(default.clone()));
            default.clone()
        } else if cfg_def.required {
            let help_suffix = cfg_def
                .help
                .as_deref()
                .map(|h| format!("\n  {h}"))
                .unwrap_or_default();
            let display_name = if cfg_def.sensitive {
                format!("{name} (sensitive)")
            } else {
                name.clone()
            };
            anyhow::bail!(
                "required config '{display_name}' not set. Use --{name} <value>{help_suffix}"
            );
        } else {
            continue;
        };
        // Mask sensitive values in all non-execution output
        if cfg_def.sensitive {
            tracing::info!("config '{}' = **** (sensitive)", name);
        } else {
            tracing::debug!("config '{}' = '{}'", name, effective_value);
        }
    }
    // Inject undeclared KEY=VALUE values as {config.xxx} — a CLI override
    // must win over the workflow's [config] table (issue #62).
    for (k, v) in cli_arg_values {
        config
            .config
            .insert(k.clone(), toml::Value::String(v.clone()));
    }
    warn_retired_cache_aging_key(config);
    Ok(())
}

/// The post-run `cache_max_age_days` environment-cache aging sweep was
/// retired in favor of `oxo-flow env gc` (issue #974) — the shared env
/// store (#904) owns environment lifecycle now. The key still PARSES (it is
/// a plain `cache_max_age_days` \[config\] entry), but it has no effect; warn once per invocation
/// so a stale workflow doesn't silently expect pruning (diagnostic ships
/// with docs: run.md "Cache environment setup").
fn warn_retired_cache_aging_key(config: &WorkflowConfig) {
    if config.config.contains_key("cache_max_age_days") {
        eprintln!(
            "{} config key 'cache_max_age_days' no longer does anything — post-run \
             environment-cache aging was retired (issue #974). Reclaim stale entries with \
             `oxo-flow env gc` (its --min-age-days gate defaults to 90).",
            "Warning:".bold().yellow(),
        );
    }
}

/// Warn about `KEY=VALUE` overrides the workflow never uses.
///
/// Undeclared override keys are a documented escape hatch (issue #62), so
/// this warns rather than fails — but a TYPO (`samplename` for
/// `sample_name`) used to pass in total silence and exit 0 having changed
/// nothing (audit finding). A key counts as used when `[config]` declares it
/// or the workflow references `{config.KEY}` anywhere; the closest declared
/// key is named when it is within a two-edit typo.
pub(crate) fn warn_unknown_override_keys(
    overrides: &HashMap<String, String>,
    declared: &HashSet<String>,
    workflow: &Path,
) {
    let unknown: Vec<&String> = overrides
        .keys()
        .filter(|key| !declared.contains(*key))
        .collect();
    if unknown.is_empty() {
        return;
    }
    // The workflow parsed a moment ago; if it cannot be re-read the
    // reference scan is skipped and every undeclared key warns.
    let text = std::fs::read_to_string(workflow).unwrap_or_default();
    for key in unknown {
        if text.contains(&format!("config.{key}")) {
            continue;
        }
        let hint = closest_declared_key(key, declared)
            .map(|candidate| format!(" — did you mean '{candidate}'?"))
            .unwrap_or_default();
        eprintln!(
            "{} override '{key}' is not a declared [config] key and the workflow never \
             references {{config.{key}}}{hint} It changes nothing.",
            "Warning:".bold().yellow(),
        );
    }
}

/// Nearest declared config key within a two-edit typo of `key` (the same
/// heuristic `oxo-flow-core`'s unknown-key check applies).
pub(crate) fn closest_declared_key<'a>(
    key: &str,
    declared: &'a HashSet<String>,
) -> Option<&'a str> {
    let lowered = key.to_ascii_lowercase();
    declared
        .iter()
        .map(|candidate| {
            (
                edit_distance(&lowered, &candidate.to_ascii_lowercase(), 2),
                candidate,
            )
        })
        .filter(|(distance, _)| *distance <= 2)
        .min_by_key(|(distance, candidate)| (*distance, candidate.as_str()))
        .map(|(_, candidate)| candidate.as_str())
}

/// Levenshtein edit distance, capped so unrelated pairs exit early.
pub(crate) fn edit_distance(a: &str, b: &str, cap: usize) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    if a.len().abs_diff(b.len()) > cap {
        return cap + 1;
    }
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut cur = vec![0usize; b.len() + 1];
    for (i, ca) in a.iter().enumerate() {
        cur[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let cost = usize::from(ca != cb);
            cur[j + 1] = (prev[j + 1] + 1).min(cur[j] + 1).min(prev[j] + cost);
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    prev[b.len()]
}

/// Nearest rule names to a typo'd `--rule` value (issue #862): a typo of an
/// existing rule/template name, or a name one of them extends (`mutect2`
/// for `mutect2_paired`), within the top `limit` picks. Both names the user
/// can legitimately pass — instance and template — live in `candidates`.
pub(crate) fn nearest_rule_names(name: &str, candidates: &[&str], limit: usize) -> Vec<String> {
    let lowered = name.to_ascii_lowercase();
    let mut scored: Vec<(usize, &str)> = candidates
        .iter()
        .filter(|candidate| **candidate != name)
        .filter_map(|candidate| {
            let cand = candidate.to_ascii_lowercase();
            if cand.starts_with(&lowered) || lowered.starts_with(&cand) {
                Some((0, *candidate))
            } else {
                let d = edit_distance(&lowered, &cand, 3);
                (d <= 3).then_some((d, *candidate))
            }
        })
        .collect();
    scored.sort_unstable_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(b.1)));
    scored.truncate(limit);
    scored
        .into_iter()
        .map(|(_, name)| name.to_string())
        .collect()
}
