//! Checkpoint re-entry helpers: DEFAULT_CHECKPOINT/SNAPSHOT_STALE_SECS, process_reentry, process_output_pattern_discovery (#948 P2-5).

use super::*;

/// Default checkpoint location used when `status` is invoked without an argument.
pub(crate) const DEFAULT_CHECKPOINT: &str = ".oxo-flow/checkpoint.json";

/// Snapshot age (seconds) at which `status` flags the checkpoint as stale
/// (issue #849). The observed flush lag on a live 561-rule run was ~1–2 min;
/// beyond this threshold the human output switches to a warning.
pub(crate) const SNAPSHOT_STALE_SECS: i64 = 300;

/// Process a checkpoint rule's re-entry manifest after it completes
/// (issue #78 P3): merge new samples, re-expand from templates, extend the
/// plan, and record the re-entry in the checkpoint. Errors fail the rule.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn process_reentry(
    config: &mut WorkflowConfig,
    rule: &Rule,
    workdir: &std::path::Path,
    wildcard_values: &std::collections::HashMap<String, String>,
    sched: &mut oxo_flow_core::scheduler::SchedulerState,
    order: &mut Vec<String>,
    order_set: &mut std::collections::HashSet<String>,
    dag: &mut WorkflowDag,
    checkpoint: &Arc<tokio::sync::Mutex<CheckpointState>>,
) -> anyhow::Result<()> {
    let manifest_path = rule.checkpoint_manifest.as_ref().ok_or_else(|| {
        anyhow::anyhow!("checkpoint rule '{}' has no checkpoint_manifest", rule.name)
    })?;
    let expanded =
        oxo_flow_core::executor::checkpoint::expand_config_in_path(manifest_path, wildcard_values);
    let full = workdir.join(&expanded);
    let content = std::fs::read_to_string(&full).map_err(|e| {
        anyhow::anyhow!(
            "checkpoint rule '{}' produced no readable manifest at '{}': {e}",
            rule.name,
            full.display()
        )
    })?;
    let manifest = oxo_flow_core::reentry::parse_manifest(&content)
        .map_err(|e| anyhow::anyhow!("checkpoint rule '{}': {e}", rule.name))?;
    if manifest.samples.is_empty() && manifest.pairs.is_empty() && manifest.values.is_empty() {
        return Ok(()); // empty manifest = valid no-op
    }
    let new_names = oxo_flow_core::reentry::apply_reentry_ext(
        config,
        manifest.group.as_deref(),
        &manifest.samples,
        &manifest.pairs,
        &manifest.values,
    )?;
    if new_names.is_empty() {
        return Ok(()); // everything already present
    }
    let round = {
        let mut ck = checkpoint.lock().await;
        let round = ck.reentries.iter().map(|r| r.round).max().unwrap_or(0) + 1;
        if round > oxo_flow_core::reentry::MAX_REENTRY_ROUNDS {
            anyhow::bail!(
                "re-entry round cap ({}) exceeded — checkpoint rule '{}' keeps discovering new values; this is a workflow bug",
                oxo_flow_core::reentry::MAX_REENTRY_ROUNDS,
                rule.name
            );
        }
        ck.record_reentry(oxo_flow_core::reentry::ReentryRecord {
            round,
            rule: rule.name.clone(),
            group: manifest.group,
            samples: manifest.samples,
            pairs: manifest.pairs,
            values: manifest.values,
        });
        round
    };
    for name in &new_names {
        sched.add_rule(name);
        order_set.insert(name.clone());
        order.push(name.clone());
    }
    *dag = WorkflowDag::from_rules_with_config(
        &config.rules,
        &config_placeholder_values(&config.config),
    )
    .context("failed to rebuild workflow DAG after re-entry")?;
    tracing::info!(
        rule = %rule.name,
        round = round,
        new_instances = ?new_names,
        "checkpoint re-entry expanded the plan"
    );
    Ok(())
}

/// Runtime-discovered output fan-out (issue #227 item 5): the producer
/// instance named `instance_name` just completed. Scan the filesystem for
/// the files its (baked) `output_pattern` enumerates, contribute them to
/// the producer template's accumulated domain, persist the domain to the
/// checkpoint FIRST (so a crash resumes without re-running the producer),
/// and — once ALL of the producer's instances are completed, so the domain
/// union is final — instantiate the deferred consumers and insert them
/// into the remaining plan. Forward-safety comes from completion ordering,
/// not DAG topology: new instances join `order`/`order_set` and the DAG is
/// rebuilt, exactly like checkpoint re-entry.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn process_output_pattern_discovery(
    config: &mut WorkflowConfig,
    instance_name: &str,
    workdir: &std::path::Path,
    sched: &mut oxo_flow_core::scheduler::SchedulerState,
    order: &mut Vec<String>,
    order_set: &mut std::collections::HashSet<String>,
    dag: &mut WorkflowDag,
    checkpoint: &Arc<tokio::sync::Mutex<CheckpointState>>,
    checkpoint_path: &std::path::Path,
) -> anyhow::Result<Vec<String>> {
    let Some(template) = config.output_pattern_template_of(instance_name) else {
        return Ok(Vec::new());
    };
    let Some(instance) = config.get_rule(instance_name).cloned() else {
        return Ok(Vec::new());
    };

    // 1. Scan the filesystem for this instance's output_pattern files.
    //
    // A fully-bound baked pattern (issue #296) is a static contract:
    // plan-time fan-out already IS the domain and plan-time consumers hold
    // their DAG edges — discovery has nothing to enumerate and the
    // zero-discovery warning would be spurious. Deferred consumers can
    // still be attached, though, when the binding came from a fan-out the
    // engine baked itself (scatter writes its variable into the pattern):
    // their domain is the instance's own bindings, not the filesystem, so
    // contribute that directly instead of starving them silently.
    let combos = if config.output_pattern_needs_discovery(&instance) {
        config.discover_output_pattern_files(&instance, workdir)?
    } else {
        let bindings = config.instance_bindings(instance_name);
        if bindings.is_empty()
            || config
                .pending_output_pattern_consumers_of(&template)
                .is_empty()
        {
            return Ok(Vec::new());
        }
        vec![bindings]
    };
    if combos.is_empty() {
        // Zero discoveries after producer success: loud, but legal — the
        // consumers stay uninstantiated (a later instance of the same
        // producer may still contribute the domain).
        eprintln!(
            "{} output_pattern '{}' of rule '{}' matched no files — \
             downstream consumers stay uninstantiated",
            "Warning:".bold().yellow(),
            instance.output_pattern.as_deref().unwrap_or_default(),
            instance_name
        );
        tracing::warn!(
            rule = instance_name,
            pattern = ?instance.output_pattern,
            "output_pattern discovery matched no files"
        );
        return Ok(Vec::new());
    }

    // 2. Contribute to the template's domain (union across instances) and
    //    persist FIRST — a crash after this point resumes without
    //    re-running the producer.
    config.contribute_output_pattern_domain(&template, combos.clone());
    {
        let mut ck = checkpoint.lock().await;
        ck.record_output_pattern_domain(&template, combos);
        if let Err(save_err) = ck.save_to_file(checkpoint_path) {
            tracing::warn!("Failed to save checkpoint: {save_err}");
        }
    }

    // 3. Gate: only instantiate consumers once ALL of the producer's
    //    instances are completed — the domain union must be final (a
    //    partial domain would drop values discovered later).
    let belongs_to = |r: &Rule| {
        r.output_pattern.is_some()
            && (r.name == template
                || config.expansion_templates.get(&r.name).map(String::as_str)
                    == Some(template.as_str()))
    };
    let outstanding = config.rules.iter().any(|r| {
        belongs_to(r) && sched.status(&r.name) != Some(oxo_flow_core::executor::JobStatus::Success)
    });
    if outstanding {
        return Ok(Vec::new());
    }

    // 4. Instantiate the deferred consumers on the final domain.
    let new_names = config.expand_output_pattern_consumers()?;
    if new_names.is_empty() {
        return Ok(Vec::new());
    }
    for name in &new_names {
        sched.add_rule(name);
        order_set.insert(name.clone());
        order.push(name.clone());
    }
    *dag = WorkflowDag::from_rules_with_config(
        &config.rules,
        &config_placeholder_values(&config.config),
    )
    .context("failed to rebuild workflow DAG after output_pattern fan-out")?;
    tracing::info!(
        producer = template,
        new_instances = ?new_names,
        "output_pattern discovery expanded the plan"
    );
    Ok(new_names)
}
