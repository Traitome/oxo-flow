//! Backend pre-flight: BackendPreflightFailure, install hints, primary env spec, backend_preflight_failures (#948 P2-5).

/// One consolidated backend-availability failure from the run pre-flight:
/// identical (backend, message) pairs across a cohort's instances collapse
/// into a single entry whose `count` says how many pending rules share it.
#[derive(Debug, PartialEq)]
pub(crate) struct BackendPreflightFailure {
    pub(crate) kind: String,
    pub(crate) message: String,
    pub(crate) example_rule: String,
    pub(crate) spec: String,
    pub(crate) count: usize,
}

impl std::fmt::Display for BackendPreflightFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{kind}: {message} — required by {count}, e.g. `{example_rule}` (environment: {spec}); {hint}",
            kind = self.kind,
            message = self.message,
            count = oxo_flow_core::plural(self.count, "pending rule", "pending rules"),
            example_rule = self.example_rule,
            spec = self.spec,
            hint = backend_install_hint(self),
        )
    }
}

/// Remediation hint for a preflight failure. The failure MODE decides the
/// advice (issue #848): a missing pixi MANIFEST is a workflow defect —
/// advise creating/correcting the declared file; a missing backend BINARY
/// is an environment defect — advise installing the tool.
pub(crate) fn backend_install_hint(failure: &BackendPreflightFailure) -> &'static str {
    if failure.kind == "pixi" && failure.message.contains("not found") {
        return "create the manifest or correct the rule's `pixi` path (resolved relative to the workflow directory)";
    }
    match failure.kind.as_str() {
        "pixi" => "install pixi (https://pixi.sh) and ensure it is on PATH",
        "mamba" => "install mamba or micromamba and ensure it is on PATH",
        "conda" => "install conda and ensure it is on PATH",
        "docker" => "install/start Docker and ensure `docker` is on PATH",
        "singularity" => "install Singularity/Apptainer and ensure it is on PATH",
        "modules" => "ensure environment modules (modulecmd) is on PATH",
        _ => "make the environment backend available on PATH",
    }
}

/// Primary spec string for an environment, for the failure message.
pub(crate) fn primary_env_spec(env: &oxo_flow_core::rule::EnvironmentSpec) -> String {
    env.mamba
        .as_deref()
        .or(env.conda.as_deref())
        .or(env.pixi.as_deref())
        .or(env.docker.as_deref())
        .or(env.singularity.as_deref())
        .or(env.venv.as_deref())
        .map(str::to_string)
        .unwrap_or_else(|| env.modules.join(","))
}

/// Validate each unique environment spec among the pending rules once
/// (instances fan out one rule template × N samples, so the spec set is
/// tiny even for large cohorts) and collapse the failures: rules sharing
/// one broken spec merge into a single entry, and distinct specs failing
/// with the identical message (the backend-missing case) merge too, so
/// the operator fixes one root cause per line. First-failure order is
/// preserved for stable output.
pub(crate) fn backend_preflight_failures(
    pending: &[(String, oxo_flow_core::rule::EnvironmentSpec)],
    resolver: &oxo_flow_core::environment::EnvironmentResolver,
) -> Vec<BackendPreflightFailure> {
    // spec cache_key → failure slot (None = spec validated OK)
    let mut spec_slot: std::collections::HashMap<String, Option<usize>> =
        std::collections::HashMap::new();
    // (backend kind, message) → failure slot, for merging identical root causes
    let mut msg_slot: std::collections::HashMap<(String, String), usize> =
        std::collections::HashMap::new();
    let mut out: Vec<BackendPreflightFailure> = Vec::new();
    for (rule_name, env) in pending {
        let key = resolver.cache_key(env);
        match spec_slot.get(&key) {
            Some(Some(slot)) => {
                out[*slot].count += 1;
                continue;
            }
            Some(None) => continue, // identical spec, already validated OK
            None => {}
        }
        if let Err(err) = resolver.validate_spec(env) {
            let (kind, message) = match &err {
                oxo_flow_core::OxoFlowError::Environment { kind, message } => {
                    (kind.clone(), message.clone())
                }
                other => ("environment".to_string(), other.to_string()),
            };
            let slot = match msg_slot.get(&(kind.clone(), message.clone())) {
                Some(&slot) => {
                    out[slot].count += 1;
                    slot
                }
                None => {
                    out.push(BackendPreflightFailure {
                        kind: kind.clone(),
                        message: message.clone(),
                        example_rule: rule_name.clone(),
                        spec: primary_env_spec(env),
                        count: 1,
                    });
                    msg_slot.insert((kind, message), out.len() - 1);
                    out.len() - 1
                }
            };
            spec_slot.insert(key, Some(slot));
        } else {
            spec_slot.insert(key, None);
        }
    }
    out
}
