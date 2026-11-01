//! CLI override parsing/validation: --arg KEY=VALUE handling, key canonicalization, unknown-key warnings, run-flag registries, retired-key diagnostics (#948 P2-5).

use super::*;

/// Long-form names of `oxo-flow run` flags (plus clap's global --help/
/// --version), derived from the `Run` subcommand definition itself so the
/// list cannot drift from main.rs — the hand-maintained copy had already
/// missed `--allow-protected-drift` (#935).
///
/// clap's trailing `config_overrides` positional (`allow_hyphen_values`)
/// cannot distinguish `--json` the run flag from `--json` a hyphen-value
/// once positional overrides start, so flags typed after `KEY=VALUE` land
/// in the override list (issue #71). Matching them here turns a confusing
/// "invalid config flag" error — or a silently wrong override — into
/// actionable guidance.
pub(crate) fn run_long_flags() -> &'static std::collections::BTreeSet<String> {
    static FLAGS: std::sync::OnceLock<std::collections::BTreeSet<String>> =
        std::sync::OnceLock::new();
    FLAGS.get_or_init(|| run_subcommand_flags().0)
}

/// Short-form run flag names (same contract as [`run_long_flags`]).
pub(crate) fn run_short_flags() -> &'static std::collections::BTreeSet<String> {
    static FLAGS: std::sync::OnceLock<std::collections::BTreeSet<String>> =
        std::sync::OnceLock::new();
    FLAGS.get_or_init(|| run_subcommand_flags().1)
}

/// One clap introspection pass backing both flag sets. `build()` first so
/// clap's implicit `--help`/`--version` flags are part of the result.
fn run_subcommand_flags() -> (
    std::collections::BTreeSet<String>,
    std::collections::BTreeSet<String>,
) {
    use clap::CommandFactory;
    let mut cmd = <crate::Cli as CommandFactory>::command();
    cmd.build();
    let run = cmd
        .find_subcommand("run")
        .expect("the Cli enum must keep a `run` subcommand");
    let mut longs: std::collections::BTreeSet<String> = run
        .get_arguments()
        .filter_map(|arg| arg.get_long())
        .map(String::from)
        .collect();
    // clap adds --version/-V only to the top-level command; subcommands
    // inherit it at parse time, so keep both classifiable here.
    longs.insert("version".to_string());
    let mut shorts = run
        .get_arguments()
        .filter_map(|arg| arg.get_short())
        .map(String::from)
        .collect::<std::collections::BTreeSet<String>>();
    shorts.insert("V".to_string());
    (longs, shorts)
}

/// Run flags that `dry-run` also accepts as real command flags (issue
/// #441: `--cache-dir` was added for run→dry-run transcription parity).
/// When the swallowed-flag parser runs under `dry-run`, these names read
/// as a misordered command flag, not "not supported".
pub(crate) const DRY_RUN_SHARED_FLAGS: &[&str] = &["cache-dir"];

/// Flags removed from `run`, mapped to their replacement.
///
/// They must NOT live in [`run_long_flags`]: that list names real command
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
            .filter(|name| run_long_flags().contains(*name))
            .map(|name| format!("--{name}"))
            .or_else(|| {
                arg_str
                    .strip_prefix('-')
                    .filter(|rest| !rest.starts_with('-'))
                    .map(|flag| flag.split('=').next().unwrap_or(flag))
                    .filter(|name| run_short_flags().contains(*name))
                    .map(|name| format!("-{name}"))
            });
        if let Some(flag) = swallowed_flag {
            // issue #430: dry-run (and every other command sharing this
            // parser) accepts only a subset of run's flags — a token like
            // `-j` under `dry-run` is not a misordered flag but one the
            // command does not support at all, so the message is
            // command-aware.
            let flag_name = flag.trim_start_matches('-');
            let is_run_flag = run_long_flags().contains(flag_name)
                || (flag_name.len() == 2 && run_short_flags().contains(&flag_name[1..]));
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

/// The CLI-override phase both `run` and `dry-run` apply in the same
/// order (issue #77): parse `--KEY VALUE` tokens against the workflow's
/// own `[config]` keys — unknown `--` tokens are rejected as typos
/// instead of silently swallowed (issue #71) — merge them into the
/// config, then warn when a key is neither declared nor referenced
/// anywhere in the workflow. `config.config` covers both plain values
/// (`key = "v"`) and declarative entries (`key = { default = ... }`);
/// `config_meta` alone only sees the latter. `command` labels the parse
/// errors ("run" / "dry-run").
pub(crate) fn apply_cli_override_phase(
    config: &mut WorkflowConfig,
    workflow: &Path,
    cli_args: Vec<String>,
    command: &str,
) -> anyhow::Result<()> {
    let declared_config_keys: HashSet<String> = config.config.keys().cloned().collect();
    let cli_arg_values = parse_cli_overrides(cli_args, &declared_config_keys, command)?;
    apply_cli_overrides(config, &cli_arg_values)?;
    warn_unknown_override_keys(&cli_arg_values, &declared_config_keys, workflow);
    Ok(())
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

#[cfg(test)]
mod tests {
    use super::*;
    use oxo_flow_core::config::WorkflowConfig;
    use std::collections::HashSet;

    fn config_with_rules(rules_toml: &str) -> WorkflowConfig {
        WorkflowConfig::parse(rules_toml).unwrap()
    }

    fn declared(keys: &[&str]) -> HashSet<String> {
        keys.iter().map(|k| k.to_string()).collect()
    }

    #[test]
    fn derived_run_flags_track_the_clap_definition() {
        // The hand-maintained copy of this list drifted: `run` gained
        // --allow-protected-drift (#935) but the list was never updated,
        // so the swallowed-flag guidance rejected it as an unknown
        // argument. Derivation closes that sync point permanently.
        let flags = run_long_flags();
        for name in [
            "jobs",
            "arg",
            "samples",
            "allow-protected-drift",
            "help",
            "version",
        ] {
            assert!(flags.contains(name), "run flags must contain --{name}");
        }
        let shorts = run_short_flags();
        for name in ["j", "k", "d", "t", "r", "h", "V"] {
            assert!(shorts.contains(name), "run shorts must contain -{name}");
        }
    }

    #[test]
    fn removed_flags_stay_out_of_the_derived_run_flags() {
        // `--sample` is retired (replacement --samples): it must not be
        // classified as a live command flag, or a config key named
        // `sample` would have its override rejected as a command flag.
        assert!(!run_long_flags().contains("sample"));
    }

    #[test]
    fn accepts_equals_forms_for_any_key() {
        let map = parse_cli_overrides(
            vec![
                "threads=8".to_string(),
                "--mode=protein".to_string(),
                "--new_key=1".to_string(), // undeclared injection via '=' stays legal
            ],
            &declared(&["threads", "mode"]),
            "run",
        )
        .unwrap();
        assert_eq!(map["threads"], "8");
        assert_eq!(map["mode"], "protein");
        assert_eq!(map["new_key"], "1");
    }

    #[test]
    fn accepts_space_form_for_declared_keys() {
        let map = parse_cli_overrides(
            vec!["--min_quality".to_string(), "45".to_string()],
            &declared(&["min_quality"]),
            "run",
        )
        .unwrap();
        assert_eq!(map["min_quality"], "45");
    }

    #[test]
    fn canonicalizes_hyphen_spelled_override_to_declared_key() {
        // issue #794: `--api-token=x` names the declared key `api_token`.
        // Without canonicalization the override is stored under the dashed
        // alias: the declared key keeps its default and the value bypasses
        // sensitive masking in the checkpoint snapshot.
        let map = parse_cli_overrides(
            vec![
                "--api-token=sk-OVERRIDE-123".to_string(),
                "min-quality=45".to_string(),
            ],
            &declared(&["api_token", "min_quality"]),
            "run",
        )
        .unwrap();
        assert_eq!(map["api_token"], "sk-OVERRIDE-123");
        assert_eq!(map["min_quality"], "45");
        assert!(!map.contains_key("api-token"), "{map:?}");
        assert!(!map.contains_key("min-quality"), "{map:?}");
    }

    #[test]
    fn canonicalizes_space_form_for_hyphen_spelled_declared_key() {
        // issue #794: `--api-token VALUE` must be accepted when only the
        // underscore-spelled `api_token` is declared.
        let map = parse_cli_overrides(
            vec!["--api-token".to_string(), "sk-OVERRIDE-123".to_string()],
            &declared(&["api_token"]),
            "run",
        )
        .unwrap();
        assert_eq!(map["api_token"], "sk-OVERRIDE-123");
    }

    #[test]
    fn canonicalization_wins_on_exact_match_and_stays_silent_on_ambiguity() {
        // Exact declared match beats normalization: `mode` declared plus
        // `sub-mode` hyphen-spelled keeps its literal key.
        let map = parse_cli_overrides(
            vec!["--sub-mode=2".to_string()],
            &declared(&["sub_mode"]),
            "run",
        )
        .unwrap();
        assert_eq!(map["sub_mode"], "2");

        // Two declared keys normalizing to the same form is pathological but
        // must not crash or silently pick one: the literal spelling is kept
        // and the usual undeclared-injection / unknown-token rules apply.
        let map = parse_cli_overrides(
            vec!["--a-b=1".to_string()],
            &declared(&["a_b", "a-b"]),
            "run",
        )
        .unwrap();
        assert_eq!(map["a-b"], "1");

        // Nothing declared matches → literal key, issue #62 injection.
        let map = parse_cli_overrides(
            vec!["--totally-unknown=7".to_string()],
            &declared(&["min_quality"]),
            "run",
        )
        .unwrap();
        assert_eq!(map["totally-unknown"], "7");

        // Space form for an unresolvable dashed token stays rejected (#71).
        let err = parse_cli_overrides(
            vec!["--totally-unknown".to_string(), "7".to_string()],
            &declared(&["min_quality"]),
            "run",
        )
        .unwrap_err();
        assert!(format!("{err}").contains("unknown argument"), "{err:?}");
    }

    #[test]
    fn apply_cli_overrides_puts_override_on_declared_sensitive_key() {
        // issue #794 end-to-end at the apply layer: the canonicalized
        // override must win over the declarative default (precedence
        // CLI > default) instead of leaving the default in place.
        let mut config = config_with_rules(
            r#"
[workflow]
name = "t"
version = "1.0"

[config]
api_token = { default = "sk-DEFAULT-abc", sensitive = true, help = "API token" }

[[rules]]
name = "r"
shell = "true"
"#,
        );
        let overrides = parse_cli_overrides(
            vec!["--api-token=sk-OVERRIDE-123".to_string()],
            &declared(&["api_token"]),
            "run",
        )
        .unwrap();
        apply_cli_overrides(&mut config, &overrides).unwrap();
        assert_eq!(config.config["api_token"].as_str(), Some("sk-OVERRIDE-123"));
    }

    #[test]
    fn override_phase_wires_parse_apply_and_command_label() {
        // Composition pin for apply_cli_override_phase: one call performs
        // the parse → apply → warn sequence both run and dry-run rely on,
        // and the command label reaches the parse errors (issue #77
        // parity — the preview must reject what execution rejects).
        let mut config = config_with_rules(
            r#"
[workflow]
name = "t"
version = "1.0"

[config]
threads = { default = "4", help = "thread count" }

[[rules]]
name = "r"
shell = "true"
"#,
        );
        apply_cli_override_phase(
            &mut config,
            std::path::Path::new("t.oxoflow"),
            vec!["--threads=8".to_string()],
            "run",
        )
        .unwrap();
        assert_eq!(config.config["threads"].as_str(), Some("8"));

        let err = apply_cli_override_phase(
            &mut config,
            std::path::Path::new("t.oxoflow"),
            vec!["--config".to_string(), "config/x.toml".to_string()],
            "dry-run",
        )
        .unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("dry-run"), "{msg}");
    }

    #[test]
    fn rejects_unknown_dash_token() {
        let err = parse_cli_overrides(
            vec!["--config".to_string(), "config/x.toml".to_string()],
            &declared(&["min_quality"]),
            "run",
        )
        .unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("unknown argument"), "{msg}");
        assert!(msg.contains("--config"), "{msg}");
        assert!(msg.contains("KEY=VALUE"), "{msg}");
    }

    #[test]
    fn rejects_known_flag_after_positional_with_flag_error() {
        // issue #71 contract: real flags swallowed by the trailing
        // positional get the "command flag, not a config override" error.
        let err = parse_cli_overrides(
            vec!["threads=8".to_string(), "--json".to_string()],
            &declared(&["threads"]),
            "run",
        )
        .unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("command flag"), "{msg}");
        assert!(msg.contains("--json"), "{msg}");
    }

    #[test]
    fn dry_run_treats_shared_run_flags_as_misordered_not_unsupported() {
        // issue #441: dry-run accepts `--cache-dir` as a real flag now, so a
        // swallowed one must read as misordered, not "not supported by
        // dry-run".
        let err = parse_cli_overrides(
            vec!["threads=8".to_string(), "--cache-dir=x".to_string()],
            &declared(&["threads"]),
            "dry-run",
        )
        .unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("command flag"), "{msg}");
        assert!(msg.contains("must come before"), "{msg}");

        // Other run flags stay "not supported" under dry-run.
        let err = parse_cli_overrides(
            vec!["threads=8".to_string(), "--provenance".to_string()],
            &declared(&["threads"]),
            "dry-run",
        )
        .unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("not supported by dry-run"), "{msg}");
    }

    #[test]
    fn rejects_space_form_for_undeclared_key_even_via_arg() {
        // `--arg --config x` lands in the same override list.
        let err = parse_cli_overrides(
            vec!["--config".to_string(), "x".to_string()],
            &declared(&[]),
            "run",
        )
        .unwrap_err();
        assert!(format!("{err}").contains("unknown argument"));
    }

    #[test]
    fn rejects_declared_key_without_value() {
        let err = parse_cli_overrides(
            vec!["--min_quality".to_string()],
            &declared(&["min_quality"]),
            "run",
        )
        .unwrap_err();
        assert!(format!("{err}").contains("invalid config flag"), "{err:?}");
    }

    #[test]
    fn rejects_space_form_that_would_swallow_a_command_flag() {
        // Audit finding: `dry-run wf --mode --json` consumed `--json` as the
        // value of the declared key `mode`, so the run silently dropped its
        // JSON output. The `=` form already errors; the space form must too.
        let err = parse_cli_overrides(
            vec!["--mode".to_string(), "--json".to_string()],
            &declared(&["mode"]),
            "run",
        )
        .unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("--mode"), "{msg}");
        assert!(msg.contains("--json"), "{msg}");
        assert!(msg.contains("--mode=<value>"), "{msg}");
    }

    #[test]
    fn ai_attempts_defaults_to_one_and_never_zero() {
        // Audit finding: the flag was accepted and discarded. It now bounds
        // the recovery/analysis attempts; absent → the pre-flag single try,
        // and an explicit 0 still means one attempt (never none).
        assert_eq!(ai_attempts(None), 1);
        assert_eq!(ai_attempts(Some(0)), 1);
        assert_eq!(ai_attempts(Some(3)), 3);
    }

    #[test]
    fn failure_exit_code_parses_recorded_reasons() {
        // The failure summary format from the executor path.
        assert_eq!(
            failure_exit_code("exit code 3 — ERROR: unknown reference genome hg999"),
            3
        );
        assert_eq!(failure_exit_code("exit code 137"), 137);
        assert_eq!(failure_exit_code("exit code 137\nstderr: ..."), 137);
        // Panics/aborts carry no code in the reason → keep -1.
        assert_eq!(failure_exit_code("task panicked: boom"), -1);
        assert_eq!(failure_exit_code(""), -1);
        assert_eq!(failure_exit_code("exit code notanumber"), -1);
    }

    #[test]
    fn closest_declared_key_names_a_typo_but_not_an_unrelated_key() {
        // Audit finding: `samplename=world` was silently accepted.
        let keys = declared(&["sample_name", "threads", "min_quality"]);
        assert_eq!(
            closest_declared_key("samplename", &keys),
            Some("sample_name")
        );
        assert_eq!(closest_declared_key("threds", &keys), Some("threads"));
        assert_eq!(closest_declared_key("completely_different", &keys), None);
    }

    #[test]
    fn rejects_bare_non_key_value_positional() {
        let err =
            parse_cli_overrides(vec!["naked".to_string()], &declared(&[]), "run").unwrap_err();
        assert!(format!("{err}").contains("KEY=VALUE"), "{err:?}");
    }

    #[test]
    fn accepts_empty_value_overrides() {
        // issue #430: `KEY=` and `--KEY ""` are legitimate derive-if-empty
        // overrides (rnaseq gene_bed/chrom_sizes/transcript_fasta branch on
        // `[ -n "{config.x}" ]`).
        let map = parse_cli_overrides(
            vec![
                "gene_bed=".to_string(),
                "--chrom_sizes".to_string(),
                String::new(),
                "--transcript_fasta=".to_string(),
            ],
            &declared(&["gene_bed", "chrom_sizes", "transcript_fasta"]),
            "run",
        )
        .unwrap();
        assert_eq!(map["gene_bed"], "");
        assert_eq!(map["chrom_sizes"], "");
        assert_eq!(map["transcript_fasta"], "");
    }

    #[test]
    fn rejects_empty_key_but_accepts_empty_value() {
        // "=value" trims to an empty KEY — still a syntax error.
        let err = parse_cli_overrides(vec!["=v".to_string()], &declared(&[]), "run").unwrap_err();
        assert!(
            format!("{err}").contains("KEY must be non-empty"),
            "{err:?}"
        );
    }

    #[test]
    fn dry_run_flag_error_is_command_aware() {
        // issue #430: `-j` under dry-run is not a misordered run flag — the
        // command does not support it at all. The message must say so
        // instead of implying a flag-ordering problem.
        let err = parse_cli_overrides(
            vec!["-j".to_string(), "12".to_string()],
            &declared(&[]),
            "dry-run",
        )
        .unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("not supported by dry-run"), "{msg}");
        assert!(
            !msg.contains("command flag, not a config override"),
            "{msg}"
        );
    }

    #[test]
    fn run_flag_error_under_run_keeps_ordering_hint() {
        // `run` itself keeps the original ordering guidance (issue #71).
        let err = parse_cli_overrides(
            vec!["threads=8".to_string(), "-j".to_string()],
            &declared(&["threads"]),
            "run",
        )
        .unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("command flag"), "{msg}");
        assert!(msg.contains("before KEY=VALUE"), "{msg}");
    }

    #[test]
    fn unknown_token_under_dry_run_stays_command_aware() {
        // An unknown --token under a non-run command points at the command,
        // not at run's ordering rule.
        let err = parse_cli_overrides(
            vec!["--config".to_string(), "x".to_string()],
            &declared(&[]),
            "dry-run",
        )
        .unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("unknown argument"), "{msg}");
        assert!(msg.contains("dry-run"), "{msg}");
    }
}
