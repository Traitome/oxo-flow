//! Reference/prune bookkeeping: rule_resource_rows, RefRecord, source_content_sig, free_kilobytes, age_ready_list, suggest_jobs (#948 P2-5).

use super::*;

/// Per-rule resource rows for the `--json` summary (issue #163): the same
/// data the report's Benchmarks table shows — wall time, sampled peak RSS,
/// sampled CPU seconds, retries — keyed by rule, deterministic order.
/// Empty for aborted paths where no checkpoint summary is meaningful.
pub(crate) fn rule_resource_rows(
    checkpoint: &oxo_flow_core::executor::checkpoint::CheckpointState,
) -> Vec<serde_json::Value> {
    let mut names: Vec<&String> = checkpoint.benchmarks.keys().collect();
    names.sort_unstable();
    names
        .into_iter()
        .map(|name| {
            let b = &checkpoint.benchmarks[name];
            // Issue #690: a stale failed_rules entry with a recorded
            // when-false verdict was gated off, not failed.
            let status = if checkpoint.completed_rules.contains(name) {
                "completed"
            } else if checkpoint.failed_rules.contains(name) {
                if checkpoint.when_verdicts.get(name) == Some(&false) {
                    "skipped_by_when"
                } else {
                    "failed"
                }
            } else {
                "running"
            };
            serde_json::json!({
                "rule": name,
                "status": status,
                "wall_time_secs": b.wall_time_secs,
                // Synthetic up-to-date records (issue #324 F-1) carry a 0.0
                // placeholder wall time — JSON consumers can distinguish
                // them from real measurements via this marker.
                "recorded_as": b.recorded_as,
                "peak_rss_mb": b.max_memory_mb,
                "cpu_seconds": b.cpu_seconds,
                "retries": b.retries,
            })
        })
        .collect()
}

/// Stored reference state: the decision fingerprint plus the source's
/// content signature (size + full content hash for small files) at the
/// time it was recorded. The signature enables the path-migration
/// exemption — identical content at a new path skips the rebuild.
#[derive(serde::Serialize, serde::Deserialize, Clone, Default)]
pub(crate) struct RefRecord {
    pub(crate) fingerprint: String,
    pub(crate) content_sig: Option<String>,
    /// The source path (config-expanded, workdir-relative) this record was
    /// computed against — the exemption re-fingerprints with it to prove a
    /// fingerprint change is a PURE path migration.
    pub(crate) source_path: Option<String>,
}

/// Content signature of a reference SOURCE: `size:<bytes>` plus
/// `|hash:<sha256>` for files under the manifest hash cap. Directories
/// and large files return `None` — the path-migration exemption requires
/// the strong hash component, so anything weaker never skips a rebuild.
pub(crate) fn source_content_sig(path: &std::path::Path) -> Option<String> {
    let md = std::fs::metadata(path).ok()?;
    if !md.is_file() {
        return None;
    }
    let mut sig = format!("size:{}", md.len());
    if let Some(hash) = oxo_flow_core::executor::checkpoint::content_hash_if_small(path, &md) {
        sig.push_str(&format!("|hash:{hash}"));
    }
    Some(sig)
}

/// Free disk space in KiB on the filesystem holding `path`, read from
/// `df -Pk` (the portable POSIX form; parses the Available column of the
/// mount line). `None` when df is unavailable or the output is unparsable —
/// the pre-flight degrades to no check rather than blocking the run.
pub(crate) fn free_kilobytes(path: &std::path::Path) -> Option<u64> {
    let out = std::process::Command::new("df")
        .args(["-Pk", path.to_str()?])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let stdout = String::from_utf8_lossy(&out.stdout);
    // The last line is the mount that actually holds the path.
    let line = stdout.lines().next_back()?;
    line.split_whitespace().nth(3)?.parse().ok()
}

/// Sorts the scheduler's ready list by effective priority: declared priority
/// plus rounds already waited (the aging counter of issue #123), ties broken
/// by name. Pure and unit-tested — the dispatch loop feeds it the live ready
/// list every round, so passed-over rules gain priority until submitted.
pub(crate) fn age_ready_list(
    mut ready: Vec<String>,
    order_set: &std::collections::HashSet<String>,
    submitted: &std::collections::HashSet<String>,
    priority_map: &std::collections::HashMap<String, i32>,
    waited: &std::collections::HashMap<String, i32>,
) -> Vec<String> {
    ready.retain(|name| order_set.contains(name) && !submitted.contains(name));
    ready.sort_by(|a, b| {
        let pa = priority_map.get(a).copied().unwrap_or(0) + waited.get(a).copied().unwrap_or(0);
        let pb = priority_map.get(b).copied().unwrap_or(0) + waited.get(b).copied().unwrap_or(0);
        pb.cmp(&pa).then_with(|| a.cmp(b))
    });
    ready
}

/// Suggest a `-j` (max concurrent jobs) value for the workflow on this
/// machine, evaluated **per parallel group** (issue #361).
///
/// For every wave: the wider of (wave width) and (cores ÷ heaviest rule in
/// that wave) is the number of jobs that wave can sustain. The suggestion
/// is the maximum across waves, so a light wide wave is never starved by
/// an unrelated heavy wave elsewhere in the DAG — the old global
/// `cores / max_threads_per_rule` formula measured 1.7–3.7× makespan loss
/// on mixed DAGs. The engine's resource pool already enforces declared
/// threads, so a generous suggestion is safe: this is a performance hint,
/// not an oversubscription gate.
pub(crate) fn suggest_jobs(
    config: &WorkflowConfig,
    groups: &[Vec<String>],
    system_threads: u32,
) -> u32 {
    let system_threads = system_threads.max(1);
    let wave_capacity = |names: &[String]| -> u32 {
        let max_threads = names
            .iter()
            .filter_map(|n| config.get_rule(n))
            .map(|r| r.effective_threads())
            .max()
            .unwrap_or(1)
            .max(1);
        (system_threads / max_threads)
            .min(names.len() as u32)
            .max(1)
    };
    groups
        .iter()
        .map(|g| wave_capacity(g))
        .max()
        .unwrap_or(1)
        .clamp(1, system_threads)
}

/// The `[[references]]` auto-build phase: materialize declared index/data
/// artifacts in the run workdir when missing or stale (fingerprint +
/// content-signature compare, with the legacy-state migrations), then
/// invalidate the rules that consume a rebuilt artifact through declared
/// inputs plus their DAG downstream — the checkpoint pre-process would
/// otherwise skip them as "already completed" without consulting input
/// freshness. Shell-only reads of a reference cannot be tracked; declare
/// reference artifacts in a rule's `input` list.
#[allow(clippy::too_many_arguments)] // phase plumbing: detection inputs + persistence + narration target
pub(crate) async fn auto_build_references(
    config: &WorkflowConfig,
    dag: &WorkflowDag,
    checkpoint: &Mutex<CheckpointState>,
    checkpoint_path: &Path,
    workdir: &Path,
    workflow_dir: &Path,
    wildcard_values: &HashMap<String, String>,
    sensitive_values: &[String],
    env_cache_dir: &Path,
    json_summary: &mut RunJsonSummary,
    skip_ref_build: bool,
) -> anyhow::Result<()> {
    if skip_ref_build || config.references.is_empty() {
        return Ok(());
    }
    let ref_workdir = workdir;

    let ref_checkpoint = checkpoint_path
        .parent()
        .unwrap_or(Path::new("."))
        .join("reference-checkpoint.json");
    // Migration: versions before this fix wrote the state one level
    // deeper (the checkpoint parent already ends in `.oxo-flow`, so
    // the old join produced `.oxo-flow/.oxo-flow/`). Move the old file
    // over once so stored fingerprints survive the upgrade.
    let legacy_ref_checkpoint = ref_checkpoint
        .parent()
        .unwrap_or(Path::new("."))
        .join(".oxo-flow/reference-checkpoint.json");
    if !ref_checkpoint.exists() && legacy_ref_checkpoint.exists() {
        let _ = std::fs::rename(&legacy_ref_checkpoint, &ref_checkpoint);
        // The rename runs under the workdir lock (acquired above); the
        // now-empty nested directory is removed so no shadow path stays
        // behind for a future reader to mistake for the real one.
        if let Some(parent) = legacy_ref_checkpoint.parent() {
            let _ = std::fs::remove_dir(parent);
        }
    }
    // name → fingerprint. Legacy checkpoints store a plain JSON array of
    // names; those entries are adopted with the current fingerprint
    // (no rebuild) — the same one-time window as rule config snapshots.
    let mut ref_state: HashMap<String, RefRecord> = if ref_checkpoint.exists() {
        let raw = std::fs::read_to_string(&ref_checkpoint).unwrap_or_default();
        serde_json::from_str::<HashMap<String, RefRecord>>(&raw)
            .ok()
            .or_else(|| {
                // Legacy formats: a plain name→fingerprint map, or a
                // bare name list — adopted with no content signature.
                serde_json::from_str::<HashMap<String, String>>(&raw)
                    .ok()
                    .map(|m| {
                        m.into_iter()
                            .map(|(k, v)| {
                                (
                                    k,
                                    RefRecord {
                                        fingerprint: v,
                                        content_sig: None,
                                        source_path: None,
                                    },
                                )
                            })
                            .collect()
                    })
            })
            .or_else(|| {
                serde_json::from_str::<Vec<String>>(&raw)
                    .map(|names| {
                        names
                            .into_iter()
                            .map(|n| {
                                (
                                    n,
                                    RefRecord {
                                        fingerprint: String::new(),
                                        content_sig: None,
                                        source_path: None,
                                    },
                                )
                            })
                            .collect::<HashMap<String, RefRecord>>()
                    })
                    .ok()
            })
            .unwrap_or_default()
    } else {
        HashMap::new()
    };

    // Artifacts actually rebuilt in THIS run — consumers (rules whose
    // declared inputs match) are invalidated afterwards.
    let mut rebuilt_outputs: Vec<String> = Vec::new();

    for ref_def in &config.references {
        let output_path = oxo_flow_core::executor::checkpoint::expand_config_in_path(
            &ref_def.output,
            wildcard_values,
        );
        let output_full = ref_workdir.join(&output_path);
        // Issue #427: the SOURCE is a workflow-shipped file — anchor it
        // to the workflow directory, not the run workdir, so existence
        // probes, fingerprints, and content signatures all see the
        // same file regardless of --workdir. (Outputs stay anchored to
        // the run workdir: they are run artifacts.)
        let resolved_source = ref_def.source.as_ref().map(|source| {
            let expanded =
                oxo_flow_core::executor::checkpoint::expand_config_in_path(source, wildcard_values);
            let path = std::path::Path::new(&expanded);
            if path.is_absolute() {
                path.to_path_buf()
            } else {
                workflow_dir.join(path)
            }
        });
        let current_fp = oxo_flow_core::config_impact::reference_fingerprint(
            ref_def,
            &config.config,
            resolved_source.as_deref(),
        );
        let stored: Option<RefRecord> = ref_state.get(&ref_def.name).cloned();
        // Content signature of the CURRENT source (size + full content
        // hash for small files). Equality with the stored signature
        // means the source is a byte-identical copy at a new path — the
        // fingerprint differs only in the path string, so a rebuild
        // would reproduce identical output (issue #142 follow-up).
        let current_sig = resolved_source.as_deref().and_then(source_content_sig);

        // Decide whether the artifact must be (re)built. The freshness
        // check comes FIRST so an mtime-only touch reports the accurate
        // reason; a fingerprint mismatch then covers definition edits,
        // referenced-config changes, and content changes the freshness
        // check cannot see (same-path, timestamp-preserving rewrites).
        let source_newer = resolved_source.as_deref().is_some_and(|p| {
            p.exists() && oxo_flow_core::executor::checkpoint::file_is_newer(p, &output_full)
        });
        let rebuild_reason = if !output_full.exists() {
            Some("output missing")
        } else if stored.as_ref().is_some_and(|r| r.fingerprint.is_empty()) {
            // Legacy entry: adopt the current fingerprint without
            // rebuilding — and PERSIST the adoption. An in-memory-only
            // adopt would leave the entry "" forever: every future run
            // would silently re-adopt the CURRENT source state and the
            // content guard could never engage.
            ref_state.insert(
                ref_def.name.clone(),
                RefRecord {
                    fingerprint: current_fp.clone(),
                    content_sig: current_sig.clone(),
                    source_path: ref_def.source.clone(),
                },
            );
            if let Some(parent) = ref_checkpoint.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let _ = std::fs::write(
                &ref_checkpoint,
                serde_json::to_string(&ref_state).unwrap_or_default(),
            );
            None
        } else if source_newer {
            Some("source is newer than output")
        } else if stored.as_ref().is_some_and(|r| r.fingerprint != current_fp) {
            // The fingerprint changed. One benign cause: a pure PATH
            // migration — the same byte content now lives at a new
            // location (live: --arg index=/new/path where /new/path is
            // a content copy). PROOF of purity: re-fingerprinting the
            // definition with the STORED source path must reproduce the
            // stored fingerprint (so the build/output/config did NOT
            // change), AND the content signatures must match with a
            // full hash component (size-only is too weak).
            let pure_path_migration = (|| {
                let rec = stored.as_ref()?;
                let old_path_str = rec.source_path.as_deref()?;
                let old_def = oxo_flow_core::config::ReferenceDef {
                    source: Some(old_path_str.to_string()),
                    ..ref_def.clone()
                };
                let fp_at_old_path = oxo_flow_core::config_impact::reference_fingerprint(
                    &old_def,
                    &config.config,
                    Some(ref_workdir.join(old_path_str)).as_deref(),
                );
                if fp_at_old_path != rec.fingerprint {
                    return None; // definition or config really changed
                }
                let cur = current_sig.as_deref()?;
                if !cur.contains("|hash:") {
                    return None; // no strong hash — never exempt
                }
                (rec.content_sig.as_deref() == Some(cur)).then_some(())
            })()
            .is_some();
            if pure_path_migration {
                tracing::info!(
                    reference = %ref_def.name,
                    "reference source moved to a new path with identical content — keeping the existing artifact"
                );
                ref_state.insert(
                    ref_def.name.clone(),
                    RefRecord {
                        fingerprint: current_fp.clone(),
                        content_sig: current_sig.clone(),
                        source_path: ref_def.source.clone(),
                    },
                );
                if let Some(parent) = ref_checkpoint.parent() {
                    let _ = std::fs::create_dir_all(parent);
                }
                let _ = std::fs::write(
                    &ref_checkpoint,
                    serde_json::to_string(&ref_state).unwrap_or_default(),
                );
                None
            } else {
                Some("definition, source content, or referenced config changed")
            }
        } else {
            None
        };

        if let Some(reason) = rebuild_reason {
            // Issue #427: the synthetic `{input}` is the reference
            // source — a workflow-shipped file. When the run workdir
            // differs from the workflow dir, render it workflow-absolute
            // so the build shell (cwd = ref_workdir) can read it.
            let source_rendered: Option<String> = ref_def.source.as_ref().map(|source| {
                let expanded = oxo_flow_core::executor::checkpoint::expand_config_in_path(
                    source,
                    wildcard_values,
                );
                if std::path::Path::new(&expanded).is_absolute() || workflow_dir == ref_workdir {
                    expanded
                } else {
                    workflow_dir.join(expanded).to_string_lossy().into_owned()
                }
            });
            let synthetic_rule = oxo_flow_core::rule::Rule {
                name: format!("ref:{}", ref_def.name),
                // `{input}` is the renderer's alias for the reference
                // source (documented in config_impact::reference_fingerprint).
                input: source_rendered
                    .as_deref()
                    .map(|s| vec![s.to_string()].into())
                    .unwrap_or_default(),
                output: vec![output_path.clone()].into(),
                ..Default::default()
            };
            let mut build_cmd = oxo_flow_core::executor::process::render_shell_command(
                &ref_def.build,
                &synthetic_rule,
                wildcard_values,
                oxo_flow_core::scheduler::detect_system_limits(),
            );
            // `{source}` is the builder-template spelling of the same
            // thing; render it too (live evidence: tcasia's STAR
            // genomeGenerate died with 'could not open genomeFastaFile:
            // {source}' — the placeholder was never substituted). The
            // path is shell-quoted — a bare splice breaks reference
            // builds whose source path contains spaces (issue #136
            // tier-2 audit).
            if let Some(expanded) = source_rendered.as_deref() {
                build_cmd = substitute_source_placeholder(&build_cmd, expanded);
            }
            // References take the same shell prelude as rules (issue #92),
            // before the environment wrapper resolves.
            build_cmd = config.defaults.apply_shell_prelude(&build_cmd);
            // References that need workflow tools (bowtie2-build, STAR
            // genomeGenerate, …) declare an `environment` — same spec as
            // `[rules.environment]`. The env is created on first use and
            // the build command runs inside it; without one, the build
            // runs in the bare system shell as before.
            if let Some(ref env) = ref_def.environment
                && !env.is_empty()
            {
                let resolver =
                    oxo_flow_core::environment::EnvironmentResolver::with_cache_dir(env_cache_dir);
                // Issue #427: file-backed env specs (`conda = "envs/x.yaml"`)
                // resolve against the workflow directory — the build shell's
                // cwd is the ref workdir, so a workflow-relative spec would
                // not resolve from there. One resolved spec feeds cache_key,
                // setup, and wrap so all three agree on the same environment.
                let resolved_env = env.resolve_relative_paths(Some(workflow_dir));
                let key = resolver.cache_key(&resolved_env);
                if !resolver.cache_is_ready(&key).await {
                    let setup = resolver.setup_command(&resolved_env)?;
                    let out = tokio::process::Command::new("sh")
                        .arg("-c")
                        .arg(&setup)
                        .current_dir(ref_workdir)
                        .output()
                        .await;
                    match out {
                        Ok(o) if o.status.success() => {
                            resolver.cache_mark_ready(&key).await;
                        }
                        Ok(o) => {
                            let stderr = String::from_utf8_lossy(&o.stderr).into_owned();
                            // Pre-execution abort — the summary still
                            // reports the failed run (issue #142 H6).
                            json_summary.emit("failed", &RunCounts::default(), vec![]);
                            return Err(anyhow::anyhow!(
                                "failed to set up the environment for reference '{}': {}",
                                ref_def.name,
                                stderr.trim()
                            ));
                        }
                        Err(e) => {
                            json_summary.emit("failed", &RunCounts::default(), vec![]);
                            return Err(anyhow::anyhow!(
                                "failed to run the environment setup for reference '{}': {e}",
                                ref_def.name
                            ));
                        }
                    }
                }
                build_cmd = resolver.wrap_command(&build_cmd, &resolved_env, None, ref_workdir)?;
            }
            eprintln!(
                "  {} Building {}: {} ({})",
                "⚙".cyan().bold(),
                ref_def.name,
                ref_def.description.as_deref().unwrap_or(&ref_def.output),
                reason
            );
            // bash first, sh fallback — mirrors the rule executor
            // (spawn_rule_shell): `set -o pipefail` in a shell prelude
            // is invalid under dash, which /bin/sh is on Debian-family
            // systems (review finding on issue #92).
            async fn run_ref_shell(
                shell: &str,
                cmd: &str,
                dir: &Path,
            ) -> std::io::Result<std::process::ExitStatus> {
                tokio::process::Command::new(shell)
                    .arg("-c")
                    .arg(cmd)
                    .current_dir(dir)
                    .status()
                    .await
            }
            let status = match run_ref_shell("bash", &build_cmd, ref_workdir).await {
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    run_ref_shell("sh", &build_cmd, ref_workdir).await
                }
                other => other,
            };
            match status {
                Ok(s) if s.success() => {
                    // Store the POST-build fingerprint: the decision
                    // fingerprint above was computed before the build
                    // ran, and a build that creates or rewrites its own
                    // source (download-then-index) changes the source
                    // state DURING the build — storing the pre-build
                    // state would mismatch on every subsequent run and
                    // rebuild forever.
                    let post_fp = oxo_flow_core::config_impact::reference_fingerprint(
                        ref_def,
                        &config.config,
                        resolved_source.as_deref(),
                    );
                    ref_state.insert(
                        ref_def.name.clone(),
                        RefRecord {
                            fingerprint: post_fp,
                            content_sig: resolved_source.as_deref().and_then(source_content_sig),
                            source_path: ref_def.source.clone(),
                        },
                    );
                    rebuilt_outputs.push(output_path.clone());
                    if let Some(parent) = ref_checkpoint.parent() {
                        let _ = std::fs::create_dir_all(parent);
                    }
                    let _ = std::fs::write(
                        &ref_checkpoint,
                        serde_json::to_string(&ref_state).unwrap_or_default(),
                    );
                    eprintln!("    {} {} built", "✓".green(), ref_def.name);
                }
                Ok(s) => {
                    anyhow::bail!(
                        "failed to build reference '{}' (exit {}). Command: {}",
                        ref_def.name,
                        s.code().unwrap_or(-1),
                        oxo_flow_core::executor::process::mask_sensitive(
                            &build_cmd,
                            sensitive_values
                        )
                    );
                }
                Err(e) => {
                    anyhow::bail!("failed to run build command for '{}': {}", ref_def.name, e);
                }
            }
        } else if stored.is_none() {
            // Output exists but not tracked — adopt as built.
            ref_state.insert(
                ref_def.name.clone(),
                RefRecord {
                    fingerprint: current_fp,
                    content_sig: current_sig,
                    source_path: ref_def.source.clone(),
                },
            );
            if let Some(parent) = ref_checkpoint.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let _ = std::fs::write(
                &ref_checkpoint,
                serde_json::to_string(&ref_state).unwrap_or_default(),
            );
        }
    }

    // A rebuilt reference invalidates the rules that consume it through
    // declared inputs (plus their DAG downstream). The checkpoint
    // pre-process would otherwise skip them as "already completed"
    // without consulting input freshness; after invalidation the
    // executor's mtime gate sees the rebuilt artifact and re-runs them.
    // Shell-only reads of a reference cannot be tracked — declare
    // reference artifacts in a rule's `input` list.
    if !rebuilt_outputs.is_empty() {
        let mut consumers: HashSet<String> = HashSet::new();
        for rule in &config.rules {
            let inputs: Vec<String> = rule
                .input
                .to_vec()
                .into_iter()
                .map(|i| {
                    oxo_flow_core::executor::checkpoint::expand_config_in_path(&i, wildcard_values)
                })
                .collect();
            if inputs.iter().any(|input| {
                !input.contains('{') && rebuilt_outputs.iter().any(|rebuilt| rebuilt == input)
            }) {
                consumers.insert(rule.name.clone());
            }
        }
        if !consumers.is_empty() {
            let mut ck = checkpoint.lock().await;
            let invalidated =
                crate::commands::run_preview::invalidate_with_downstream(&mut ck, dag, &consumers);
            if let Err(e) = ck.save_to_file(checkpoint_path) {
                tracing::warn!(error = %e, "failed to save checkpoint after reference rebuild");
            }
            eprintln!(
                "  {} reference rebuild invalidated {}: {}",
                "↻".yellow(),
                oxo_flow_core::plural(invalidated.len(), "rule", "rules"),
                invalidated.join(", ")
            );
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use oxo_flow_core::config::WorkflowConfig;
    use std::collections::HashSet;

    fn config_with_rules(rules_toml: &str) -> WorkflowConfig {
        WorkflowConfig::parse(rules_toml).unwrap()
    }

    #[test]
    fn suggest_jobs_evaluates_each_wave_against_its_own_heaviest_rule() {
        // Waves: 20×t1, 2×t16, 20×t1 on a 64-core box. The old global
        // formula suggested 4; the per-group form keeps the wide light
        // waves at 20 (issue #361 measured 11.0 s vs 3.0 s makespan on
        // this exact shape).
        let mut toml = String::from("[workflow]\nname = \"t\"\nversion = \"0.1\"\n");
        for i in 0..20 {
            toml.push_str(&format!(
                "\n[[rules]]\nname = \"a{i}\"\noutput = [\"a{i}.txt\"]\nshell = \"true\"\n\n[rules.resources]\nthreads = 1\n"
            ));
        }
        for i in 0..2 {
            toml.push_str(&format!(
                "\n[[rules]]\nname = \"b{i}\"\noutput = [\"b{i}.txt\"]\nshell = \"true\"\n\n[rules.resources]\nthreads = 16\n"
            ));
        }
        for i in 0..20 {
            toml.push_str(&format!(
                "\n[[rules]]\nname = \"c{i}\"\noutput = [\"c{i}.txt\"]\nshell = \"true\"\n\n[rules.resources]\nthreads = 1\n"
            ));
        }
        let config = config_with_rules(&toml);
        let groups = vec![
            (0..20).map(|i| format!("a{i}")).collect::<Vec<_>>(),
            (0..2).map(|i| format!("b{i}")).collect::<Vec<_>>(),
            (0..20).map(|i| format!("c{i}")).collect::<Vec<_>>(),
        ];
        assert_eq!(suggest_jobs(&config, &groups, 64), 20);
        // The old global formula would have said 4 here.
        assert_ne!(suggest_jobs(&config, &groups, 64), 4);
    }

    #[test]
    fn suggest_jobs_clamps_to_the_wave_width_and_system_threads() {
        // A single wave of 5 heavy rules on 4 cores: 4/16 → 0 → clamped
        // to at least 1 job, and never above the wave width (5).
        let mut toml = String::from("[workflow]\nname = \"t\"\nversion = \"0.1\"\n");
        for i in 0..5 {
            toml.push_str(&format!(
                "\n[[rules]]\nname = \"h{i}\"\noutput = [\"h{i}.txt\"]\nshell = \"true\"\n\n[rules.resources]\nthreads = 16\n"
            ));
        }
        let config = config_with_rules(&toml);
        let groups = vec![(0..5).map(|i| format!("h{i}")).collect::<Vec<_>>()];
        assert_eq!(suggest_jobs(&config, &groups, 4), 1);
        // Wide light wave on a big machine: the 16-job legacy ceiling is gone.
        let mut toml = String::from("[workflow]\nname = \"t\"\nversion = \"0.1\"\n");
        for i in 0..20 {
            toml.push_str(&format!(
                "\n[[rules]]\nname = \"l{i}\"\noutput = [\"l{i}.txt\"]\nshell = \"true\"\n\n[rules.resources]\nthreads = 1\n"
            ));
        }
        let config = config_with_rules(&toml);
        let groups = vec![(0..20).map(|i| format!("l{i}")).collect::<Vec<_>>()];
        assert_eq!(suggest_jobs(&config, &groups, 64), 20);
    }

    #[test]
    fn suggest_jobs_defaults_to_one_without_rules() {
        let config = config_with_rules("[workflow]\nname = \"t\"\nversion = \"0.1\"\n");
        assert_eq!(suggest_jobs(&config, &[], 64), 1);
        assert_eq!(suggest_jobs(&config, &[vec![]], 64), 1);
    }

    fn map_of(pairs: &[(&str, i32)]) -> std::collections::HashMap<String, i32> {
        pairs.iter().map(|(k, v)| (k.to_string(), *v)).collect()
    }

    fn set_of(names: &[&str]) -> HashSet<String> {
        names.iter().map(|n| n.to_string()).collect()
    }

    #[test]
    fn age_ready_list_orders_by_declared_priority_then_name() {
        // Arrange
        let priority = map_of(&[("high", 20), ("low", 10)]);
        // Act
        let ready = age_ready_list(
            vec!["low".into(), "high".into()],
            &set_of(&["high", "low"]),
            &HashSet::new(),
            &priority,
            &std::collections::HashMap::new(),
        );
        // Assert
        assert_eq!(ready, vec!["high", "low"]);
    }

    #[test]
    fn age_ready_list_aged_rule_overtakes_higher_declared_priority() {
        // The starvation cure (issue #123): a producer passed over for many
        // rounds must eventually outrank the higher-priority rules that keep
        // re-occupying the slots.
        // Arrange
        let priority = map_of(&[("merge", 20), ("dump", 10)]);
        let waited = map_of(&[("dump", 15)]);
        // Act
        let ready = age_ready_list(
            vec!["merge".into(), "dump".into()],
            &set_of(&["merge", "dump"]),
            &HashSet::new(),
            &priority,
            &waited,
        );
        // Assert
        assert_eq!(ready, vec!["dump", "merge"]);
    }

    #[test]
    fn age_ready_list_drops_submitted_and_out_of_scope_rules() {
        // Arrange
        let priority = map_of(&[("a", 5), ("b", 5), ("c", 5)]);
        let submitted = set_of(&["b"]);
        // Act
        let ready = age_ready_list(
            vec!["c".into(), "b".into(), "a".into()],
            &set_of(&["a", "b", "c"]),
            &submitted,
            &priority,
            &std::collections::HashMap::new(),
        );
        // Assert — b is already submitted, so it cannot be dispatched again.
        assert_eq!(ready, vec!["a", "c"]);
    }

    #[test]
    fn age_ready_list_treats_missing_priority_and_waits_as_zero() {
        // Arrange — neither rule appears in the maps.
        // Act
        let ready = age_ready_list(
            vec!["z".into(), "a".into()],
            &set_of(&["a", "z"]),
            &HashSet::new(),
            &std::collections::HashMap::new(),
            &std::collections::HashMap::new(),
        );
        // Assert — equal effective priority (0), name tie-break.
        assert_eq!(ready, vec!["a", "z"]);
    }
}
