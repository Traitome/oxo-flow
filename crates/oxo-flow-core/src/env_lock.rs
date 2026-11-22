//! Environment-lock identity recorded in run provenance (issue #1274, design
//! bet 4 "cryptographic provenance").
//!
//! ## Why this exists
//!
//! The preregistered cross-architecture contrast (audit
//! `docs/superpowers/audits/cross-arch-provenance-2026-11-19.md`, PR-01 §C4)
//! requires the two observation arms to share an *identical environment lock*
//! before any architecture claim can be made. The first field observation was
//! OS-confounded **by construction**: the `system` environment backend supplies
//! NEITHER an environment-lock hash NOR a container image digest, so nothing
//! was held constant across hosts and the primary result is INVALID as claimed.
//!
//! This module is the in-repo prerequisite that makes a future two-arm re-run
//! possible: it records a per-rule *environment-lock identity* in the run
//! checkpoint, so a re-run can prove or deny environment-lock equality instead
//! of assuming it. **No two-arm measurement is claimed here** — this is the
//! recording layer only.
//!
//! ## Identity, per backend
//!
//! - **container** (`docker`/`singularity`): the resolved image *digest*
//!   (`repo@sha256:...`), not just the tag, when the runtime can report one
//!   offline. When it cannot, the explicit sentinel [`ENV_LOCK_UNRESOLVED`] is
//!   recorded — a digest is **never fabricated**.
//! - **env backends** (`pixi`/`conda`/`mamba`/`venv`): a deterministic
//!   SHA-256 *lock hash* over the resolved environment spec files (the locked
//!   package set / manifest, including the sibling `pixi.lock`). The file set
//!   is key-ordered so the hash is stable across hosts.
//! - **`system`** (and any rule with no lockable environment content): the
//!   explicit sentinel [`ENV_LOCK_UNAVAILABLE`] — a caller can detect it. A
//!   constant hash over an empty fingerprint would be *misleading* evidence of
//!   equality, so we do not manufacture one.
//!
//! Every recorded value is a self-describing `"<kind>|<value>"` string:
//!
//! ```text
//! docker|biocontainers/bwa@sha256:abcd...      resolved container digest
//! docker|unresolved                            container digest not obtainable
//! pixi|sha256:abcd...                          env lock hash
//! system|unavailable                            backend supplies no lock
//! ```
//!
//! Use [`env_lock_is_resolved`] to test whether a recorded value carries real
//! lock content.
//!
//! ## Reuse
//!
//! The spec-file enumeration and the content hashing are NOT retyped here:
//! `crate::config_impact::environment_spec_files` enumerates the same
//! pixi/conda/mamba/venv manifests (and the pixi sibling lockfile) the rule
//! fingerprint already reads, and
//! [`crate::executor::checkpoint::compute_file_checksum`] is the canonical
//! streaming SHA-256 used for output integrity. The NUL-framed hashing loop
//! mirrors `config_impact::rule_fingerprint_impl`.

use std::collections::BTreeMap;
use std::path::Path;

use sha2::{Digest, Sha256};

/// Sentinel: the environment lock exists but could not be resolved offline
/// (e.g. a container image whose digest the runtime cannot report without a
/// pull). A caller can test for it rather than trusting a fabricated digest.
pub const ENV_LOCK_UNRESOLVED: &str = "unresolved";

/// Sentinel: the backend supplies no lockable environment content at all (the
/// `system` backend, or a rule with no declared environment). Recorded
/// explicitly so a caller can tell "no lock" apart from "lock not resolved".
pub const ENV_LOCK_UNAVAILABLE: &str = "unavailable";

/// Whether a recorded environment-lock value carries real lock content (a
/// resolved container digest or a lock hash) rather than one of the two
/// explicit sentinels. Cross-arch equality can only be asserted for resolved
/// values.
#[must_use]
pub fn env_lock_is_resolved(value: &str) -> bool {
    !value.ends_with(ENV_LOCK_UNRESOLVED) && !value.ends_with(ENV_LOCK_UNAVAILABLE)
}

/// Compute the per-rule environment-lock identity for a workflow, keyed by
/// rule name. Runs entirely offline and never fails the caller: a spec file it
/// cannot read, or a container digest it cannot resolve, degrades to an
/// explicit sentinel.
///
/// `workflow_path` is the workflow file; environment spec paths resolve
/// relative to its parent directory (the same base `collect_software_versions`
/// uses). `None` keeps spec strings only — file-backed specs degrade to
/// [`ENV_LOCK_UNRESOLVED`].
#[must_use]
pub fn compute_env_lock_identity(
    config: &crate::config::WorkflowConfig,
    workflow_path: Option<&Path>,
) -> BTreeMap<String, String> {
    compute_env_lock_identity_with(config, workflow_path, &default_container_digest)
}

/// Testable core of [`compute_env_lock_identity`]: the container-digest source
/// is injected so the container branch can be exercised deterministically
/// without a container runtime.
fn compute_env_lock_identity_with(
    config: &crate::config::WorkflowConfig,
    workflow_path: Option<&Path>,
    container_digest: &dyn Fn(&str, &str) -> String,
) -> BTreeMap<String, String> {
    let base_dir = workflow_path.map(crate::parent_dir);
    let mut out = BTreeMap::new();
    for rule in &config.rules {
        let env = config.resolve_environment(rule).unwrap_or_default();
        out.insert(
            rule.name.clone(),
            rule_env_lock_identity(&env, base_dir, container_digest),
        );
    }
    out
}

fn rule_env_lock_identity(
    env: &crate::rule::EnvironmentSpec,
    base_dir: Option<&Path>,
    container_digest: &dyn Fn(&str, &str) -> String,
) -> String {
    let kind = env.kind();
    let mut pairs: Vec<(String, String)> = Vec::new();

    if let Some(base) = base_dir {
        for (label, path) in crate::config_impact::environment_spec_files(env, base) {
            let digest = crate::executor::checkpoint::compute_file_checksum(&path)
                .unwrap_or_else(|_| ENV_LOCK_UNRESOLVED.to_string());
            pairs.push((label, digest));
        }
    }
    if !env.modules.is_empty() {
        // HPC modules are part of the environment; their declared order is
        // semantic (load order), so it is preserved rather than sorted.
        pairs.push(("modules".to_string(), env.modules.join(",")));
    }

    let container = env
        .docker
        .as_deref()
        .map(|spec| ("docker", spec))
        .or_else(|| env.singularity.as_deref().map(|spec| ("singularity", spec)));

    // A pure-container rule's identity IS the resolved digest — not a hash of
    // a hash — matching the two-arm comparison the audit calls for.
    if pairs.is_empty() {
        if let Some((backend, spec)) = container {
            let digest = resolve_or_sentinel(container_digest(backend, spec));
            return format!("{backend}|{digest}");
        }
        return format!("{kind}|{ENV_LOCK_UNAVAILABLE}");
    }

    if let Some((backend, spec)) = container {
        let digest = resolve_or_sentinel(container_digest(backend, spec));
        pairs.push((backend.to_string(), digest));
    }

    // If any component is an explicit sentinel, the rule's lock is NOT fully
    // resolved. Surface that as a detectable sentinel rather than baking the
    // placeholder text into a hash: two hosts that each failed to resolve a
    // *different* spec would otherwise hash to the same `sha256:...` value and
    // read as equal evidence — a false cross-arch match.
    if pairs.iter().any(|(_, value)| !env_lock_is_resolved(value)) {
        return format!("{kind}|{ENV_LOCK_UNRESOLVED}");
    }

    format!("{kind}|sha256:{}", lock_hash(&pairs))
}

fn resolve_or_sentinel(resolved: String) -> String {
    if resolved.trim().is_empty() {
        ENV_LOCK_UNRESOLVED.to_string()
    } else {
        resolved
    }
}

/// Default container-digest resolver: best-effort, offline, never pulls.
fn default_container_digest(backend: &str, spec: &str) -> String {
    match backend {
        "docker" => docker_image_digest(spec),
        // singularity/apptainer: no reliable offline digest query; the caller
        // sees the explicit sentinel rather than a fabricated one.
        _ => String::new(),
    }
}

/// Local, offline `docker image inspect` digest query. Returns the empty
/// string when the image is absent or the daemon is unreachable — it never
/// pulls (a pull is a network side effect the recording layer must not have).
fn docker_image_digest(spec: &str) -> String {
    let out = std::process::Command::new("docker")
        .args([
            "image",
            "inspect",
            "--format",
            "{{if .RepoDigests}}{{index .RepoDigests 0}}{{end}}",
            spec,
        ])
        .output();
    match out {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout).trim().to_string(),
        _ => String::new(),
    }
}

/// Key-ordered lock hash over `(label, value)` pairs: sorts a copy so
/// insertion/host order never leaks into the digest (the persisted-artifact
/// order-independence invariant).
fn lock_hash(pairs: &[(String, String)]) -> String {
    let mut sorted: Vec<(&str, &str)> = pairs
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect();
    sorted.sort();
    nulframed_sha256(&sorted)
}

/// NUL-framed SHA-256 over label/value pairs — the same unambiguous framing
/// `config_impact::rule_fingerprint_impl` uses.
fn nulframed_sha256(pairs: &[(&str, &str)]) -> String {
    let mut hasher = Sha256::new();
    for (label, content) in pairs {
        hasher.update(label.as_bytes());
        hasher.update([0u8]);
        hasher.update(content.as_bytes());
        hasher.update([0u8]);
    }
    hex::encode(hasher.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a config through the canonical TOML parse path from a `[[rules]]`
    /// fragment, so the fixtures exercise the real resolver, not hand-built
    /// structs.
    fn config_with(rules_toml: &str) -> crate::config::WorkflowConfig {
        let toml = format!("[workflow]\nname = \"t\"\nversion = \"1.0\"\n\n{rules_toml}");
        crate::config::WorkflowConfig::parse(&toml).unwrap()
    }

    // (a) determinism: the same resolved spec hashes to the same identity.
    #[test]
    fn lock_identity_is_deterministic() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("env.yaml"),
            "dependencies:\n  - bwa=0.7.17\n",
        )
        .unwrap();
        let cfg = config_with(
            "[[rules]]\nname = \"align\"\nshell = \"true\"\n\n[rules.environment]\nconda = \"env.yaml\"\n",
        );
        let wf = dir.path().join("wf.oxoflow");

        let a = compute_env_lock_identity(&cfg, Some(&wf));
        let b = compute_env_lock_identity(&cfg, Some(&wf));
        assert_eq!(a, b, "lock identity must be deterministic");
        assert!(a.contains_key("align"));
        assert!(env_lock_is_resolved(a.get("align").unwrap()));
    }

    // (b) key-order independence: the lock hash must not depend on pair order.
    #[test]
    fn lock_hash_is_order_independent() {
        let forward = lock_hash(&[
            (
                "environment_file:conda:env.yaml".to_string(),
                "sha256:aa".to_string(),
            ),
            ("modules".to_string(), "gcc/11.2".to_string()),
        ]);
        let reverse = lock_hash(&[
            ("modules".to_string(), "gcc/11.2".to_string()),
            (
                "environment_file:conda:env.yaml".to_string(),
                "sha256:aa".to_string(),
            ),
        ]);
        assert_eq!(
            forward, reverse,
            "the lock hash must be independent of pair insertion order"
        );
    }

    // (c) sensitive to the resolved spec: editing an in-place spec file must
    // change the identity (falsifiable — if the content were ignored, equal).
    #[test]
    fn lock_identity_changes_when_resolved_spec_changes() {
        let dir = tempfile::tempdir().unwrap();
        let manifest = dir.path().join("pixi.toml");
        // The pixi env spec includes the sibling pixi.lock, so a fully resolved
        // pixi lock requires both files present.
        std::fs::write(&manifest, "[dependencies]\nbwa = \"0.7.17\"\n").unwrap();
        std::fs::write(
            dir.path().join("pixi.lock"),
            "[[package]]\nname = \"bwa\"\nversion = \"0.7.17\"\n",
        )
        .unwrap();
        let cfg = config_with(
            "[[rules]]\nname = \"align\"\nshell = \"true\"\n\n[rules.environment]\npixi = \"pixi.toml\"\n",
        );
        let wf = dir.path().join("wf.oxoflow");

        let before = compute_env_lock_identity(&cfg, Some(&wf));
        assert!(
            env_lock_is_resolved(before.get("align").unwrap()),
            "fixture must resolve: {:?}",
            before
        );
        std::fs::write(&manifest, "[dependencies]\nbwa = \"0.7.18\"\n").unwrap();
        let after = compute_env_lock_identity(&cfg, Some(&wf));
        assert_ne!(
            before, after,
            "an in-place spec edit must change the environment-lock identity"
        );
    }

    // The two explicit sentinels: container digest unresolved, system
    // unavailable. Uses an injected resolver so the container branch is
    // exercised deterministically without a container runtime.
    #[test]
    fn sentinels_injected_container_and_system() {
        let cfg = config_with(
            "[[rules]]\nname = \"d1\"\nshell = \"true\"\n\n[rules.environment]\ndocker = \"biocontainers/bwa:0.7.17\"\n\n[[rules]]\nname = \"s1\"\nshell = \"true\"\n",
        );

        let with_digest: &dyn Fn(&str, &str) -> String =
            &|_backend, _spec| "biocontainers/bwa@sha256:deadbeef".to_string();
        let resolved = compute_env_lock_identity_with(&cfg, None, with_digest);
        assert_eq!(
            resolved.get("d1").unwrap(),
            "docker|biocontainers/bwa@sha256:deadbeef"
        );
        assert!(env_lock_is_resolved(resolved.get("d1").unwrap()));
        assert_eq!(resolved.get("s1").unwrap(), "system|unavailable");
        assert!(!env_lock_is_resolved(resolved.get("s1").unwrap()));

        // A resolver that cannot report a digest must yield the explicit
        // sentinel, never a fabricated value.
        let empty: &dyn Fn(&str, &str) -> String = &|_b, _s| String::new();
        let unresolved = compute_env_lock_identity_with(&cfg, None, empty);
        assert_eq!(unresolved.get("d1").unwrap(), "docker|unresolved");
        assert!(!env_lock_is_resolved(unresolved.get("d1").unwrap()));
    }

    // The field lands in the serialized checkpoint, and stays out of the JSON
    // when empty (legacy-skip convention).
    #[test]
    fn env_lock_identity_lands_in_serialized_checkpoint() {
        let mut state = crate::executor::CheckpointState::new();
        assert!(
            !serde_json::to_string(&state)
                .unwrap()
                .contains("env_lock_identity"),
            "an empty map must not serialize (legacy-skip convention)"
        );

        let mut map = BTreeMap::new();
        map.insert("align".to_string(), "docker|unresolved".to_string());
        state.set_env_lock_identity(map);

        let json = serde_json::to_string(&state).unwrap();
        assert!(json.contains("env_lock_identity"));
        let back: crate::executor::CheckpointState = serde_json::from_str(&json).unwrap();
        assert_eq!(
            back.env_lock_identity.get("align").unwrap(),
            "docker|unresolved"
        );
    }

    // A rule with only a bare (non-file) conda env name has no lockable content
    // resolvable offline -> the spec path is unreadable -> explicit sentinel.
    #[test]
    fn bare_conda_name_degrades_to_sentinel() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = config_with(
            "[[rules]]\nname = \"r\"\nshell = \"true\"\n\n[rules.environment]\nconda = \"myenv\"\n",
        );
        let wf = dir.path().join("wf.oxoflow");
        let map = compute_env_lock_identity(&cfg, Some(&wf));
        let value = map.get("r").unwrap();
        assert!(
            value.contains(ENV_LOCK_UNRESOLVED),
            "unreadable/bare spec must degrade to the unresolved sentinel, got {value}"
        );
    }
}
