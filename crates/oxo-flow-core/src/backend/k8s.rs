//! Kubernetes batch/v1 Job manifest generator.
//!
//! Maps ScheduledRule resources and container specifications onto Kubernetes
//! batch/v1 Job manifests with deterministic labels. Submission/polling goes
//! through the shared `ClusterExecutor` (`backend/cluster.rs`).

use super::{BackendJobStatus, sanitize_cloud_name, scheduled_resources};
use crate::cluster::ClusterJobConfig;
use crate::config::ClusterVolume;
use crate::environment::mirrored_spec;
use crate::error::Result;
use crate::executor::process::sensitive_env_refs;
use crate::rule::Rule;
use serde_json::{Value, json};
use std::collections::BTreeSet;

/// Extract the `cd '<path>' &&` workdir prefix the shared plan builder folds
/// into every rule command. Kubernetes Jobs run in a pod with its own
/// filesystem, so the run workdir must be mounted at the SAME absolute path
/// for the rule's declared outputs (and the engine's checkpoint/log files) to
/// land on the shared host tree. Returns `None` when the prefix is absent
/// (backends/plans that do not fold `cd`, e.g. hand-written commands).
pub(crate) fn workdir_from_shell_cmd(shell_cmd: &str) -> Option<String> {
    // The plan builder escapes a literal quote as '\'' (four shell bytes:
    // quote, backslash, quote, quote) inside the single-quoted path.
    const ESCAPED_QUOTE: &str = "'\\''";
    let rest = shell_cmd.strip_prefix("cd '")?;
    let mut out = String::new();
    let mut index = 0usize;
    while index < rest.len() {
        if rest[index..].starts_with(ESCAPED_QUOTE) {
            out.push('\'');
            index += ESCAPED_QUOTE.len();
            continue;
        }
        if rest.as_bytes()[index] == b'\'' {
            rest[index + 1..].strip_prefix(" && ")?;
            return Some(out);
        }
        let ch = rest[index..].chars().next()?;
        out.push(ch);
        index += ch.len_utf8();
    }
    None
}
/// Mount the run workdir (and any additional absolute host directory the
/// rendered command reads — today the array dispatch line's per-chunk
/// command directory) at its host path, for the single-node /
/// shared-filesystem cluster mode that is the documented Kubernetes
/// deployment. PVC/NFS deployments pin their own mounts instead.
///
/// Relative paths are skipped: a `hostPath` must be absolute, and a relative
/// one would be rejected by the API server with a confusing validation
/// error. Entries keep their order, so single-job manifests keep their
/// historical shape (workdir = mount #0) byte-comparable.
fn attach_host_path_mounts(container: &mut Value, entries: &[(&'static str, String)]) {
    let volume_mounts: Vec<Value> = entries
        .iter()
        .filter(|(_, path)| path.starts_with('/'))
        .map(|(name, path)| json!({ "name": name, "mountPath": path }))
        .collect();
    if !volume_mounts.is_empty() {
        container["volumeMounts"] = json!(volume_mounts);
    }
}

/// The `volumes` array matching [`attach_host_path_mounts`] (same order,
/// same names) for the pod spec.
fn host_path_volumes(entries: &[(&'static str, String)]) -> Option<Value> {
    let volumes: Vec<Value> = entries
        .iter()
        .filter(|(_, path)| path.starts_with('/'))
        .map(|(name, path)| {
            json!({
                "name": name,
                "hostPath": { "path": path, "type": "Directory" }
            })
        })
        .collect();
    if volumes.is_empty() {
        None
    } else {
        Some(json!(volumes))
    }
}

/// The node's container-daemon socket, when the operator opted in via
/// `OXO_FLOW_K8S_DOCKER_SOCKET=1`. A FALLBACK channel for jobs whose command
/// reaches the node daemon — a non-renderable environment (local SIF,
/// conda/pixi/venv/modules) that folds its local wrapper into the command, or
/// a `system` rule that calls docker itself. Issue #1388 renders docker specs
/// and singularity `docker://` URIs as the pod's own image, and an
/// image-rendered pod never receives this mount: it has no use for a daemon
/// and must not be granted host-root-equivalent access.
fn docker_socket_mount() -> Option<(&'static str, &'static str)> {
    match std::env::var("OXO_FLOW_K8S_DOCKER_SOCKET").as_deref() {
        Ok("1" | "true" | "yes" | "on") => Some(("oxoflow-docker-sock", "/var/run/docker.sock")),
        _ => None,
    }
}

/// Append the docker-socket channel to a container: volume mount (hostPath
/// type `Socket` — a different resource type than the directory mounts) and
/// the `DOCKER_HOST` env var. Appended AFTER `attach_sensitive_env` so the
/// env entry is added without disturbing the Secret projections.
fn attach_docker_socket(container: &mut Value, pod_volumes: &mut Vec<Value>) {
    let Some((name, path)) = docker_socket_mount() else {
        return;
    };
    let mounts = container
        .as_object_mut()
        .expect("container is an object")
        .entry("volumeMounts")
        .or_insert_with(|| json!([]));
    mounts.as_array_mut().expect("array").push(json!({
        "name": name, "mountPath": path
    }));
    pod_volumes.push(json!({
        "name": name,
        "hostPath": { "path": path, "type": "Socket" }
    }));
    let env = container
        .as_object_mut()
        .expect("container is an object")
        .entry("env")
        .or_insert_with(|| json!([]));
    env.as_array_mut().expect("array").push(json!({
        "name": "DOCKER_HOST", "value": format!("unix://{path}")
    }));
}

/// Sanitise a string for use as a Kubernetes resource name or label value.
/// RFC 1123 label: lowercase alphanumeric characters or minus (-), max 63 chars.
pub fn sanitize_k8s_name(name: &str) -> String {
    sanitize_cloud_name(name, 63, true, "")
}

/// Deterministic chunk/run-scoped suffix for a generated K8s array job name.
///
/// `seed` is the per-chunk job directory (`.../jobs/<template>/chunk-<k>`),
/// so the digest is stable for a given chunk on re-render (idempotent) and
/// differs across chunks of one template and across runs. Two chunks of the
/// same template no longer render the same `metadata.name` and collide on
/// `kubectl create` (issue #1226).
fn array_name_suffix(seed: &str) -> String {
    use sha2::{Digest, Sha256};
    // 8 hex chars is ample to separate the handful of concurrent chunks of
    // one template, and short enough to leave the rule name room under the
    // RFC 1123 63-char cap.
    hex::encode(Sha256::digest(seed.as_bytes()))[..8].to_string()
}

/// Kubernetes Secret the manifest pulls `sensitive = true` config values
/// from (issue #1120 / ledger RB-12). The operator creates it once per
/// workflow, keyed by the same `OXO_FLOW_SENSITIVE_<n>` variable names the
/// local executor routes values under, so a manifest never carries a secret
/// literal in `args`.
///
/// ```sh
/// kubectl create secret generic oxoflow-secrets \
///   --from-literal=OXO_FLOW_SENSITIVE_0=<value>
/// ```
pub const K8S_SENSITIVE_SECRET: &str = "oxoflow-secrets";

/// Attach the secret-backed `env` array to a container block: every sensitive
/// value referenced by the rendered command is projected out of the
/// operator-provided Kubernetes Secret rather than inlined as a literal.
/// A container whose command carries no routed sensitive reference is left
/// untouched, so ordinary jobs stay byte-identical.
///
/// The Secret key IS the environment variable name: the plan-level routing
/// numbers bindings once for the whole plan (`sensitive_bindings`), so the
/// name is unambiguous across rules and across array elements — a per-rule
/// key namespace would break indexed jobs, whose elements are named after
/// the template rather than the instance.
fn attach_sensitive_env(container: &mut Value, refs: &[String]) {
    let env: Vec<Value> = refs
        .iter()
        .map(|var| {
            json!({
                "name": var,
                "valueFrom": {
                    "secretKeyRef": {
                        "name": K8S_SENSITIVE_SECRET,
                        "key": var,
                    }
                }
            })
        })
        .collect();
    if !env.is_empty() {
        container["env"] = json!(env);
    }
}

/// Build the container resource block shared by single and array jobs:
/// cpu/memory requests, and cpu/memory limits plus the optional NVIDIA GPU
/// limit (`gpu` or `gpu_spec`). Array jobs previously dropped the GPU
/// limit, silently scheduling GPU rules onto CPU-only nodes.
fn container_resources(rule: &Rule) -> serde_json::Value {
    let (threads, mem_mb) = scheduled_resources(rule);

    let mut limits = json!({
        "cpu": format!("{threads}"),
        "memory": format!("{mem_mb}Mi"),
    });

    if let Some(gpu_count) = rule.resources.gpu {
        if gpu_count > 0 {
            limits["nvidia.com/gpu"] = json!(format!("{gpu_count}"));
        }
    } else if let Some(spec) = &rule.resources.gpu_spec
        && spec.count > 0
    {
        limits["nvidia.com/gpu"] = json!(format!("{}", spec.count));
    }

    json!({
        "requests": {
            "cpu": format!("{threads}"),
            "memory": format!("{mem_mb}Mi"),
        },
        "limits": limits,
    })
}

/// The pod `volumes` entry for the `[cluster.volume]` declaration: NFS, a
/// bound PVC, or the single-node hostPath. Mounted at `mount_root`.
fn shared_volume_entry(volume: &ClusterVolume, name: &str) -> serde_json::Value {
    let source = match volume.kind.as_str() {
        "nfs" => json!({ "nfs": { "server": volume.server, "path": volume.path } }),
        "pvc" => json!({ "persistentVolumeClaim": { "claimName": volume.claim_name } }),
        // Validated at profile admission; render a Directory hostPath.
        _ => json!({ "hostPath": { "path": volume.host_path, "type": "Directory" } }),
    };
    let mut entry = json!({ "name": name });
    entry
        .as_object_mut()
        .expect("object")
        .extend(source.as_object().expect("object").clone());
    entry
}

/// Rewrite the submit-host workdir paths in a rendered command to the
/// in-pod mount root: the folded `cd '<workdir>'` prefix and every
/// workdir-anchored absolute path (docker `-v` binds, output redirections
/// use relative paths) must point INSIDE the mounted volume. Replacing the
/// exact workdir string is deterministic — it is one distinct absolute path
/// — and `mount_root`/…`/` substitution happens before the manifest renders
/// (expand-before-derive: the checkpoint keeps the submit-host path; only
/// the RENDERED command is mapped).
pub(crate) fn rewrite_workdir_to_mount_root(
    shell_cmd: &str,
    workdir: &str,
    mount_root: &str,
) -> String {
    shell_cmd
        .replace(&format!("{workdir}/"), &format!("{mount_root}/"))
        .replace(&format!("cd '{workdir}'"), &format!("cd '{mount_root}'"))
}

/// Whether Kubernetes renders a renderable container environment as the pod's
/// own image (issue #1388).
///
/// Default on. `OXO_FLOW_K8S_IMAGE_RENDER=0|false|no|off` forces the
/// pre-#1388 wrapper-fallback path for every rule on the box — the escape
/// hatch for a docker rule that needs host paths outside the workdir (pair it
/// with `OXO_FLOW_K8S_DOCKER_SOCKET=1`) or for an operator who must keep the
/// old behavior. A truthy spelling keeps image rendering on; an unrecognized
/// value warns and stays ON (the documented default), so a typo can never
/// silently change execution semantics either way.
pub fn image_rendering_enabled() -> bool {
    image_rendering_enabled_with(std::env::var("OXO_FLOW_K8S_IMAGE_RENDER").ok().as_deref())
}

/// Pure form of [`image_rendering_enabled`] for tests.
fn image_rendering_enabled_with(raw: Option<&str>) -> bool {
    let Some(raw) = raw else {
        return true;
    };
    if raw.trim().is_empty() {
        return true; // an empty value is unset
    }
    match raw.trim().to_ascii_lowercase().as_str() {
        "0" | "false" | "no" | "off" => false,
        "1" | "true" | "yes" | "on" => true,
        other => {
            tracing::warn!(
                value = other,
                "OXO_FLOW_K8S_IMAGE_RENDER is unrecognized — image rendering stays on"
            );
            true
        }
    }
}

/// The image a Kubernetes Job runs for `rule`, plus the renderable image
/// reference when the rule's container environment is rendered as that image
/// (issue #1388).
///
/// A renderable environment (docker spec, or a singularity `docker://` URI's
/// underlying image) becomes the pod image and the job runs the wrapper-free
/// command. Everything else keeps the historical behavior: the local
/// environment wrapper stays folded into the command and the pod runs it in
/// the declared docker image (or `ubuntu:latest` when none is declared).
/// `OXO_FLOW_K8S_IMAGE_RENDER=0` forces this fallback for every rule.
///
/// Callers must pass the WRAPPER-FREE command for a renderable environment —
/// [`crate::backend::ExecutorBackend::submitted_cmd`] is the single place that
/// decision is made. Registry mirrors are applied here, at render time only;
/// the rule's declared spec keeps one identity across mirrored and direct
/// boxes.
fn pod_environment(rule: &Rule) -> (String, Option<crate::environment::ContainerImageSpec>) {
    let renderable = if image_rendering_enabled() {
        crate::environment::renderable_container_image(&rule.environment)
    } else {
        None
    };
    match renderable {
        Some(image) => (mirrored_spec(&image.spec), Some(image)),
        None => (
            mirrored_spec(
                rule.environment
                    .docker
                    .as_deref()
                    .unwrap_or("ubuntu:latest"),
            ),
            None,
        ),
    }
}

/// Annotations that make the environment-rendering decision visible in the
/// rendered Job (issue #1388).
///
/// `oxoflow.io/environment-render` is `image` when the rule's container
/// environment IS the pod image, `wrapper-fallback` when the local
/// environment wrapper stayed folded into the command, and `system` for a
/// rule that declares no environment at all (no wrapper exists to fall back
/// to). Mirror rewriting is transport-only, so `environment-image` records
/// the workflow-declared spec (the identity), not the mirrored ref the
/// `image:` field carries.
fn environment_annotations(
    rule: &Rule,
    renderable: Option<&crate::environment::ContainerImageSpec>,
) -> serde_json::Value {
    match renderable {
        Some(image) => json!({
            "oxoflow.io/environment-render": crate::executor::checkpoint::ClusterImageRecord::IMAGE,
            "oxoflow.io/environment-kind": image.kind,
            "oxoflow.io/environment-image": image.spec,
        }),
        None if rule.environment.kind() == "system" => json!({
            "oxoflow.io/environment-render": crate::executor::checkpoint::ClusterImageRecord::SYSTEM,
            "oxoflow.io/environment-kind": "system",
        }),
        None => json!({
            "oxoflow.io/environment-render":
                crate::executor::checkpoint::ClusterImageRecord::WRAPPER_FALLBACK,
            "oxoflow.io/environment-kind": rule.environment.kind(),
        }),
    }
}

/// One warning per rendered Job whose declared environment cannot run as the
/// pod image: the folded wrapper needs that runtime inside the pod. A
/// `system` rule declares no environment, so its fallback is the documented
/// deployment mode, not a degradation — no warning for it.
fn warn_wrapper_fallback(rule: &Rule) {
    let kind = rule.environment.kind();
    if kind == "system" {
        return;
    }
    // The operator's explicit opt-out is not an impossibility: say which one
    // happened (review M3).
    if !image_rendering_enabled()
        && crate::environment::renderable_container_image(&rule.environment).is_some()
    {
        tracing::warn!(
            rule = %rule.name,
            environment = kind,
            "Kubernetes: image rendering is disabled by OXO_FLOW_K8S_IMAGE_RENDER — the folded \
             local environment wrapper is submitted instead (wrapper-fallback)"
        );
        return;
    }
    tracing::warn!(
        rule = %rule.name,
        environment = kind,
        "Kubernetes: the rule's environment cannot be rendered as a pod image — the local \
         environment wrapper stays folded into the job command (wrapper-fallback); the pod's \
         own image must carry that runtime to execute it"
    );
}

/// Generate a Kubernetes batch/v1 Job manifest for a single rule.
pub fn generate_k8s_job_manifest(
    rule: &Rule,
    shell_cmd: &str,
    cluster_config: &ClusterJobConfig,
) -> Result<String> {
    // A run/submission scope (the run directory or `cluster submit` output
    // directory) is folded into the name so two runs of the same workflow do
    // not collide on AlreadyExists (issue #1158 live campaign: the second
    // `run --profile k8s` failed with `jobs.batch "oxoflow-make" already
    // exists`). No scope keeps the historical stable name.
    let base = sanitize_cloud_name(&rule.name, 40, true, "");
    let job_name = match cluster_config.name_scope.as_deref() {
        Some(scope) if !scope.is_empty() => {
            use sha2::{Digest, Sha256};
            let suffix = hex::encode(Sha256::digest(format!("{scope}|{}", rule.name).as_bytes()))
                [..8]
                .to_string();
            format!("oxoflow-{base}-{suffix}")
        }
        _ => format!("oxoflow-{base}"),
    };

    // Image-based environment rendering (issue #1388): a renderable
    // container environment IS the pod image and `shell_cmd` is then the
    // wrapper-free command (the caller selected it through
    // `ExecutorBackend::submitted_cmd`). Otherwise the historical shape
    // applies — the folded local wrapper runs in the declared image.
    let (container_image, renderable) = pod_environment(rule);
    if renderable.is_none() {
        warn_wrapper_fallback(rule);
    }

    // Shell contract (issue #1388): the local docker/singularity wrapper ran
    // the rule through CONTAINER_BASH_SHIM — bash when the image has it, the
    // image's POSIX `sh` otherwise — so an image-rendered pod must do the
    // same: `/bin/sh` always exists (busybox/alpine included), and the shim
    // selects bash when available (preludes such as `set -o pipefail` rely on
    // it). The fallback path keeps the historical `/bin/bash -c`.
    let job_command = if renderable.is_some() {
        json!(["/bin/sh", "-c"])
    } else {
        json!(["/bin/bash", "-c"])
    };
    let mut container = json!({
        "name": sanitize_k8s_name(&rule.name),
        "image": container_image,
        // Local/single-node clusters preload images (kind/k3s `load`); an
        // implicit `Always` pull on `:latest` would need a registry the
        // compute node may not reach.
        "imagePullPolicy": "IfNotPresent",
        "command": job_command,
        "args": [shell_cmd],
        "resources": container_resources(rule),
    });
    attach_sensitive_env(&mut container, &sensitive_env_refs(shell_cmd));
    let workdir = workdir_from_shell_cmd(shell_cmd);
    // [cluster.volume] mode: the workdir lives on a SHARED volume the pod
    // mounts at mount_root — the rendered command is rewritten onto the
    // mount and NO submit-host hostPath is emitted (multi-node clusters;
    // issue #1387). Without the declaration the historical single-node
    // hostPath mount applies.
    let (volume, volume_workdir) = match (&cluster_config.volume, &workdir) {
        (Some(volume), Some(workdir)) => match volume.in_pod_path(workdir) {
            // Expand-before-derive: the volume is mounted by its ROOT, so
            // the workdir appears at mount_root/<relative> — mapping the
            // workdir directly onto mount_root (the first draft) pointed
            // every path one directory short, and the pod died on `cd`.
            Some(dir) => (Some(volume), dir),
            None => {
                return Err(crate::error::OxoFlowError::Config {
                    message: format!(
                        "[cluster.volume] the run workdir '{workdir}' is not under the \
                         declared volume root — point volume_root (nfs: the export; pvc: \
                         claim mount point) at the directory containing it"
                    ),
                });
            }
        },
        _ => (None, String::new()),
    };
    let rendered_cmd = match (&volume, &workdir) {
        (Some(_), Some(workdir)) => {
            rewrite_workdir_to_mount_root(shell_cmd, workdir, &volume_workdir)
        }
        _ => shell_cmd.to_string(),
    };
    container["args"] = if renderable.is_some() {
        json!([crate::environment::CONTAINER_BASH_SHIM, "sh", rendered_cmd])
    } else {
        json!([rendered_cmd])
    };
    let mut pod_volumes: Vec<Value> = Vec::new();
    match &volume {
        Some(volume) => {
            container["volumeMounts"] = json!([
                { "name": "oxoflow-shared", "mountPath": volume.mount_root() }
            ]);
            pod_volumes.push(shared_volume_entry(volume, "oxoflow-shared"));
        }
        None => {
            let mut directory_entries: Vec<(&'static str, String)> = Vec::new();
            if let Some(workdir) = &workdir {
                directory_entries.push(("oxoflow-workdir", workdir.clone()));
            }
            attach_host_path_mounts(&mut container, &directory_entries);
            pod_volumes = host_path_volumes(&directory_entries)
                .map(|v| v.as_array().expect("array").clone())
                .unwrap_or_default();
        }
    }
    // The daemon socket exists only for the WRAPPER FALLBACK (issue #1388):
    // an image-rendered pod needs no daemon, so mounting a host-root-
    // equivalent socket into it would grant access with no use.
    if renderable.is_none() {
        attach_docker_socket(&mut container, &mut pod_volumes);
    }

    let mut pod_spec = json!({
        "restartPolicy": "Never",
        "containers": [container]
    });
    if !pod_volumes.is_empty() {
        pod_spec["volumes"] = json!(pod_volumes);
    }

    let manifest = json!({
        "apiVersion": "batch/v1",
        "kind": "Job",
        "metadata": {
            "name": job_name,
            "labels": {
                "oxoflow.io/managed-by": "oxo-flow",
                "oxoflow.io/rule": sanitize_k8s_name(&rule.name),
            },
            "annotations": environment_annotations(rule, renderable.as_ref()),
        },
        "spec": {
            "backoffLimit": 0,
            "template": {
                "metadata": {
                    "labels": {
                        "oxoflow.io/managed-by": "oxo-flow",
                        "oxoflow.io/rule": sanitize_k8s_name(&rule.name),
                    }
                },
                "spec": pod_spec
            }
        }
    });

    Ok(serde_json::to_string_pretty(&manifest)?)
}

/// Generate a Kubernetes indexed Job manifest for an array submission.
///
/// `workdir` is the run directory the per-index command files fold a
/// `cd '<workdir>' &&` guard to (the driver's `ScheduledRule.workdir`). Both
/// it and the command directory are mounted at their host paths — the pod
/// needs the command files to dispatch AND the workdir for the commands'
/// declared inputs/outputs to land on the shared host tree (the 2026-11-24
/// live campaign residual: only the workdir was mounted, so array
/// submission on Kubernetes referenced a command directory the pod could
/// not see). Relative paths are skipped (hostPath must be absolute).
pub fn generate_k8s_array_job_manifest(
    rule: &Rule,
    cmd_dir: &str,
    count: usize,
    // Union of `OXO_FLOW_SENSITIVE_<n>` references over every per-index
    // command file — the dispatch line carries none of them itself.
    sensitive_refs: &[String],
    workdir: Option<&str>,
    _cluster_config: &ClusterJobConfig,
) -> Result<String> {
    // The job name carries a deterministic per-chunk suffix so sibling
    // chunks of one template (and repeat runs) get distinct names and do not
    // collide on `kubectl create` (issue #1226). The rule name is budgeted so
    // the whole name stays within the RFC 1123 63-char cap:
    // 14 ("oxoflow-array-") + 40 (rule) + 1 ("-") + 8 (suffix) = 63.
    let job_name = format!(
        "oxoflow-array-{}-{}",
        sanitize_cloud_name(&rule.name, 40, true, ""),
        array_name_suffix(cmd_dir)
    );

    let (container_image, renderable) = pod_environment(rule);
    if renderable.is_none() {
        warn_wrapper_fallback(rule);
    }

    // Shell contract (issue #1388): an image-rendered pod starts the
    // dispatcher with `/bin/sh` (always present) and runs each per-index
    // command file through CONTAINER_BASH_FILE_SHIM, so a bash-bearing image
    // keeps its prelude semantics (`set -o pipefail`, `[[ ]]`) while a
    // bash-less image (busybox/alpine) still starts. The fallback keeps the
    // historical `/bin/bash -c` dispatcher.
    let job_command = if renderable.is_some() {
        json!(["/bin/sh", "-c"])
    } else {
        json!(["/bin/bash", "-c"])
    };
    let mut container = json!({
        "name": sanitize_k8s_name(&rule.name),
        "image": container_image,
        "imagePullPolicy": "IfNotPresent",
        "command": job_command,
        "args": [],
        "resources": container_resources(rule),
    });
    attach_sensitive_env(&mut container, sensitive_refs);
    let mut pod_volumes: Vec<Value> = Vec::new();
    // [cluster.volume] mode: both the command directory and the run workdir
    // live on the shared volume; the dispatch path is rewritten onto the
    // in-pod mount root and no submit-host hostPath is emitted (issue
    // #1387, multi-node clusters).
    let volume = _cluster_config.volume.as_ref();
    let dispatch_dir = match (volume, workdir) {
        (Some(volume), Some(_)) => {
            // The command directory lives under the run workdir, which lives
            // under the volume root; map it through the same relative-path
            // derivation as the single-job `cd` rewrite.
            volume
                .in_pod_path(cmd_dir)
                .ok_or_else(|| crate::error::OxoFlowError::Config {
                    message: format!(
                        "[cluster.volume] the command directory '{cmd_dir}' is not under the \
                         declared volume root"
                    ),
                })?
        }
        _ => cmd_dir.to_string(),
    };
    let dispatch_cmd = if renderable.is_some() {
        format!(
            "INDEX=$((JOB_COMPLETION_INDEX + 1)); sh -c '{}' sh \"{dispatch_dir}/cmd.${{INDEX}}.sh\"",
            crate::environment::CONTAINER_BASH_FILE_SHIM
        )
    } else {
        format!("INDEX=$((JOB_COMPLETION_INDEX + 1)); sh \"{dispatch_dir}/cmd.${{INDEX}}.sh\"")
    };
    container["args"] = json!([dispatch_cmd]);
    match volume {
        Some(volume) => {
            container["volumeMounts"] = json!([
                { "name": "oxoflow-shared", "mountPath": volume.mount_root() }
            ]);
            pod_volumes.push(shared_volume_entry(volume, "oxoflow-shared"));
        }
        None => {
            let mut directory_entries: Vec<(&'static str, String)> =
                vec![("oxoflow-cmds", cmd_dir.to_string())];
            if let Some(workdir) = workdir {
                // The command files fold `cd '<workdir>' &&` themselves;
                // without the mount the `cd` fails and every element dies
                // before its first tool.
                if workdir.starts_with('/') && workdir != cmd_dir {
                    directory_entries.push(("oxoflow-workdir", workdir.to_string()));
                }
            }
            attach_host_path_mounts(&mut container, &directory_entries);
            pod_volumes = host_path_volumes(&directory_entries)
                .map(|v| v.as_array().expect("array").clone())
                .unwrap_or_default();
        }
    }
    // The daemon socket exists only for the WRAPPER FALLBACK (issue #1388):
    // an image-rendered pod needs no daemon, so mounting a host-root-
    // equivalent socket into it would grant access with no use.
    if renderable.is_none() {
        attach_docker_socket(&mut container, &mut pod_volumes);
    }

    let mut pod_spec = json!({
        "restartPolicy": "Never",
        "containers": [container]
    });
    if !pod_volumes.is_empty() {
        pod_spec["volumes"] = json!(pod_volumes);
    }

    let manifest = json!({
        "apiVersion": "batch/v1",
        "kind": "Job",
        "metadata": {
            "name": job_name,
            "labels": {
                "oxoflow.io/managed-by": "oxo-flow",
                "oxoflow.io/rule": sanitize_k8s_name(&rule.name),
            },
            "annotations": environment_annotations(rule, renderable.as_ref()),
        },
        "spec": {
            "completions": count,
            "parallelism": count.min(64),
            "completionMode": "Indexed",
            "backoffLimit": 0,
            "template": {
                "metadata": {
                    "labels": {
                        "oxoflow.io/managed-by": "oxo-flow",
                        "oxoflow.io/rule": sanitize_k8s_name(&rule.name),
                    }
                },
                "spec": pod_spec
            }
        }
    });

    Ok(serde_json::to_string_pretty(&manifest)?)
}

/// Parse a Kubernetes index list — the `status.completedIndexes` /
/// `status.failedIndexes` format — into the index set it names.
///
/// The wire format is a comma-separated list of single indexes and
/// inclusive ranges (`"0-2,4"`). Malformed segments are ignored rather
/// than guessed: a garbage status field must leave the element unknown,
/// never invent a completed index (issue #1132 / ledger RB-06).
pub fn parse_index_list(spec: &str) -> BTreeSet<u32> {
    let mut indexes = BTreeSet::new();
    for part in spec.split(',') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        match part.split_once('-') {
            Some((start, end)) => {
                if let (Ok(start), Ok(end)) =
                    (start.trim().parse::<u32>(), end.trim().parse::<u32>())
                    && start <= end
                {
                    indexes.extend(start..=end);
                }
            }
            None => {
                if let Ok(index) = part.parse::<u32>() {
                    indexes.insert(index);
                }
            }
        }
    }
    indexes
}

/// The terminal phase of an Indexed Job, read from
/// `status.conditions[*].type` (`Failed` / `Complete`).
///
/// With `backoffLimit: 0` a single pod failure terminally fails the Job and
/// the controller never creates the remaining pods (`parallelism` caps the
/// in-flight count), so indexes that appear in neither status list on a
/// terminal-Failed Job will never run — they must settle, or the driver
/// polls a dead Job forever (an element verdict of `Running` resets the
/// driver's blind-settlement guard). A `Complete` Job with an unrecorded
/// index is contradictory wire data, not a provable outcome: the index is
/// [`BackendJobStatus::Unknown`] — never a fabricated success or failure —
/// and the driver's terminal-unknown guard settles it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IndexedJobTerminal {
    /// `status.conditions` contains `type: Failed`.
    Failed,
    /// `status.conditions` contains `type: Complete`.
    Complete,
}

/// The verdict an Indexed Job's status proves for one completion index.
///
/// `completed` / `failed` come from
/// [`parse_index_list`] over `status.completedIndexes` /
/// `status.failedIndexes`; `failed_count` is `status.failed`. A failed
/// count with no `failedIndexes` data (the field is Kubernetes 1.31+)
/// proves the Job is terminal-failed but not WHICH element failed, so
/// that element is [`BackendJobStatus::Unknown`] — never a success and
/// never a fabricated failure. An element that is neither recorded
/// completed nor recorded failed, while the Job is not in that blind
/// state, is still live — unless the Job itself is terminal
/// ([`IndexedJobTerminal`]): an unrecorded index can then never start, so
/// it settles instead of reporting live forever.
pub fn indexed_element_status(
    completed: &BTreeSet<u32>,
    failed: &BTreeSet<u32>,
    failed_count: u64,
    index: u32,
    job_terminal: Option<IndexedJobTerminal>,
) -> BackendJobStatus {
    if completed.contains(&index) {
        return BackendJobStatus::Completed;
    }
    if failed.contains(&index) {
        return BackendJobStatus::Failed;
    }
    if failed_count > 0 && failed.is_empty() {
        return BackendJobStatus::Unknown;
    }
    match job_terminal {
        // backoffLimit: 0 — the controller will never create or restart
        // this index; the work will not be done, so the element fails.
        Some(IndexedJobTerminal::Failed) => BackendJobStatus::Failed,
        // A Complete Job must have run every index; a missing index on a
        // Complete Job is wire data we cannot interpret — settle as
        // unknown, never fabricate the outcome.
        Some(IndexedJobTerminal::Complete) => BackendJobStatus::Unknown,
        None => BackendJobStatus::Running,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cluster::ClusterBackend;
    use crate::rule::Rule;
    use serde_json::Value;

    fn k8s_test_config() -> ClusterJobConfig {
        ClusterJobConfig {
            backend: crate::cluster::ClusterBackend::Kubernetes,
            queue: None,
            account: None,
            walltime: None,
            extra_args: Vec::new(),
            name_scope: None,
            volume: None,
        }
    }

    #[test]
    fn job_manifest_mounts_the_folded_workdir_at_its_host_path() {
        let rule = Rule {
            name: "hello".to_string(),
            output: crate::rule::FilePatterns::List(vec!["hello.txt".to_string()]),
            shell: Some("echo hi > hello.txt".to_string()),
            ..Default::default()
        };
        let cmd = "cd '/tmp/run work' && echo hi > hello.txt";
        let manifest: Value = serde_json::from_str(
            &generate_k8s_job_manifest(&rule, cmd, &k8s_test_config()).unwrap(),
        )
        .unwrap();
        let container = &manifest["spec"]["template"]["spec"]["containers"][0];
        assert_eq!(container["volumeMounts"][0]["mountPath"], "/tmp/run work");
        assert_eq!(
            manifest["spec"]["template"]["spec"]["volumes"][0]["hostPath"]["path"],
            "/tmp/run work"
        );
        assert_eq!(
            manifest["spec"]["template"]["spec"]["volumes"][0]["hostPath"]["type"],
            "Directory"
        );

        // A command without the folded `cd` keeps the previous manifest
        // shape (no volumes) — never an empty hostPath.
        let bare: Value = serde_json::from_str(
            &generate_k8s_job_manifest(&rule, "echo hi > hello.txt", &k8s_test_config()).unwrap(),
        )
        .unwrap();
        assert!(
            bare["spec"]["template"]["spec"].get("volumes").is_none(),
            "{bare}"
        );
    }

    #[test]
    fn job_names_are_scoped_per_run_so_reruns_do_not_collide() {
        let rule = Rule {
            name: "make".to_string(),
            output: crate::rule::FilePatterns::List(vec!["a.txt".to_string()]),
            shell: Some("true".to_string()),
            ..Default::default()
        };
        let cmd = "cd '/tmp/w' && true";
        let name_for = |scope: Option<&str>| -> String {
            let mut config = k8s_test_config();
            config.name_scope = scope.map(str::to_string);
            let manifest: Value =
                serde_json::from_str(&generate_k8s_job_manifest(&rule, cmd, &config).unwrap())
                    .unwrap();
            manifest["metadata"]["name"].as_str().unwrap().to_string()
        };
        assert_eq!(name_for(None), "oxoflow-make");
        let first = name_for(Some("/runs/2026-11-24T15-00"));
        let second = name_for(Some("/runs/2026-11-24T15-05"));
        assert_ne!(first, second, "two run scopes must not share a Job name");
        assert!(first.starts_with("oxoflow-make-"), "{first}");
        assert!(first.len() <= 63, "{first}");
    }

    #[test]
    fn workdir_parser_handles_single_quote_escapes() {
        assert_eq!(
            workdir_from_shell_cmd("cd '/tmp/a' && true").as_deref(),
            Some("/tmp/a")
        );
        assert_eq!(
            workdir_from_shell_cmd("cd '/tmp/it'\\''s' && true").as_deref(),
            Some("/tmp/it's")
        );
        assert_eq!(workdir_from_shell_cmd("true"), None);
    }

    fn test_config() -> ClusterJobConfig {
        ClusterJobConfig {
            backend: ClusterBackend::Kubernetes,
            queue: None,
            account: None,
            walltime: None,
            extra_args: Vec::new(),
            name_scope: None,
            volume: None,
        }
    }

    #[test]
    fn test_sanitize_k8s_name() {
        assert_eq!(sanitize_k8s_name("align_reads.1"), "align-reads-1");
        assert_eq!(sanitize_k8s_name("STAR-genome_index"), "star-genome-index");
        assert_eq!(sanitize_k8s_name("---test---"), "test");
        assert_eq!(sanitize_k8s_name(""), "job");
    }

    #[test]
    fn test_generate_k8s_job_manifest() {
        let resources = crate::rule::Resources {
            threads: 8,
            memory: Some("16G".into()),
            gpu: Some(1),
            ..Default::default()
        };
        let rule = Rule {
            name: "align_sample".into(),
            resources,
            ..Default::default()
        };

        let manifest_str =
            generate_k8s_job_manifest(&rule, "bwa mem ref.fa reads.fq", &test_config())
                .expect("manifest generation must succeed");
        let val: Value = serde_json::from_str(&manifest_str).expect("manifest must be valid json");

        assert_eq!(val["apiVersion"], "batch/v1");
        assert_eq!(val["kind"], "Job");
        assert_eq!(val["metadata"]["name"], "oxoflow-align-sample");
        assert_eq!(val["spec"]["backoffLimit"], 0);

        let container = &val["spec"]["template"]["spec"]["containers"][0];
        assert_eq!(container["resources"]["limits"]["cpu"], "8");
        assert_eq!(container["resources"]["limits"]["nvidia.com/gpu"], "1");
        assert_eq!(container["command"][0], "/bin/bash");
        assert_eq!(container["args"][0], "bwa mem ref.fa reads.fq");
    }

    #[test]
    fn test_generate_k8s_array_job_manifest() {
        let resources = crate::rule::Resources {
            threads: 4,
            memory: Some("8G".into()),
            gpu: Some(2),
            ..Default::default()
        };
        let rule = Rule {
            name: "scatter_qc".into(),
            resources,
            ..Default::default()
        };

        let manifest_str =
            generate_k8s_array_job_manifest(&rule, "run/cmds", 5, &[], None, &test_config())
                .expect("manifest generation must succeed");
        let val: Value = serde_json::from_str(&manifest_str).expect("manifest must be valid json");

        assert_eq!(val["spec"]["completions"], 5);
        assert_eq!(val["spec"]["completionMode"], "Indexed");
        assert_eq!(val["spec"]["backoffLimit"], 0);

        let container = &val["spec"]["template"]["spec"]["containers"][0];
        assert!(
            container["args"][0]
                .as_str()
                .unwrap()
                .contains("JOB_COMPLETION_INDEX")
        );
        // Array jobs must carry the same GPU limit as single jobs.
        assert_eq!(container["resources"]["limits"]["nvidia.com/gpu"], "2");

        // Absolute paths are mounted (command dir first, workdir second so
        // single-job manifests keep workdir = mount #0); relative hostPath
        // values are skipped — the API server would reject them.
        let mounted = generate_k8s_array_job_manifest(
            &rule,
            "/abs/run/jobs/t/chunk-1",
            2,
            &[],
            Some("/abs/run"),
            &test_config(),
        )
        .unwrap();
        let val: Value = serde_json::from_str(&mounted).unwrap();
        let container = &val["spec"]["template"]["spec"]["containers"][0];
        let pod_spec = &val["spec"]["template"]["spec"];
        assert_eq!(
            container["volumeMounts"][0]["mountPath"], "/abs/run/jobs/t/chunk-1",
            "the dispatch line reads the command dir — it must be mounted"
        );
        assert_eq!(container["volumeMounts"][0]["name"], "oxoflow-cmds");
        assert_eq!(container["volumeMounts"][1]["mountPath"], "/abs/run");
        assert_eq!(container["volumeMounts"][1]["name"], "oxoflow-workdir");
        assert_eq!(
            pod_spec["volumes"][0]["hostPath"]["path"],
            "/abs/run/jobs/t/chunk-1"
        );
        assert_eq!(pod_spec["volumes"][0]["hostPath"]["type"], "Directory");
        assert_eq!(pod_spec["volumes"][1]["hostPath"]["path"], "/abs/run");

        // The live-campaign residual: without a workdir (relative or unset)
        // only the command directory is mounted — never a bogus relative
        // hostPath.
        let unmounted = generate_k8s_array_job_manifest(
            &rule,
            "run/cmds",
            2,
            &[],
            Some("relative/run"),
            &test_config(),
        )
        .unwrap();
        let val: Value = serde_json::from_str(&unmounted).unwrap();
        assert!(
            val["spec"]["template"]["spec"].get("volumes").is_none(),
            "relative paths must not render hostPath volumes"
        );
    }

    /// Registry-restricted nodes (the china-mirrors residual): the
    /// manifest's image field is rewritten through the same
    /// `OXO_REGISTRY_MIRRORS` map the singularity/apptainer/docker pulls use
    /// — transport-only, the workflow never changes. A child process is
    /// required because `std::env::set_var` is unsafe in edition 2024 and
    /// this crate is `#![forbid(unsafe_code)]` (the
    /// [`crate::backend::assert_cloud_body_ignores_submit_host_env`]
    /// precedent): the parent re-executes this test binary with the mapping
    /// injected.
    #[test]
    fn k8s_manifests_rewrite_images_through_the_registry_mirror_map() {
        run_k8s_env_probe("MIRROR_DOCKER");
        run_k8s_env_probe("MIRROR_OTHER");
    }

    /// `OXO_FLOW_K8S_DOCKER_SOCKET=1` mounts the node daemon socket with the
    /// `Socket` hostPath type and exports `DOCKER_HOST` — the short-term
    /// channel for docker-environment rules. Opt-in: unset (and any
    /// unrecognized value) leaves the manifest without the channel. Same
    /// child-process pattern as the mirror probe.
    #[test]
    fn docker_socket_channel_is_opt_in_and_off_by_default() {
        run_k8s_env_probe("SOCKET_ON");
        run_k8s_env_probe("SOCKET_OFF");
    }

    /// `OXO_FLOW_K8S_IMAGE_RENDER=0` forces the pre-#1388 wrapper fallback for
    /// a renderable environment (issue #1388 review M3): same rule, fallback
    /// manifest, wrapper preserved, `/bin/bash` invocation, and no image
    /// identity claimed. Same child-process pattern as the mirror probe.
    #[test]
    fn image_render_switch_forces_the_wrapper_fallback() {
        run_k8s_env_probe("IMAGE_RENDER_OFF");
    }

    fn run_k8s_env_probe(probe: &str) {
        let exe = std::env::current_exe().expect("test binary path");
        let mut cmd = std::process::Command::new(exe);
        cmd.args([
            "--exact",
            "backend::k8s::tests::k8s_env_probe_child",
            "--ignored",
            "--test-threads=1",
            "--nocapture",
        ])
        .env("K8S_ENV_PROBE", probe)
        // Empty means unset for both knobs (mirrored_spec treats an empty
        // value as unset; the socket gate matches only truthy spellings).
        .env(
            "OXO_REGISTRY_MIRRORS",
            match probe {
                "MIRROR_DOCKER" => "docker.io=mirror.example.com",
                "MIRROR_OTHER" => "quay.io=quay-mirror.example.com",
                _ => "",
            },
        )
        .env(
            "OXO_FLOW_K8S_DOCKER_SOCKET",
            if probe == "SOCKET_ON" { "1" } else { "typo" },
        )
        .env(
            "OXO_FLOW_K8S_IMAGE_RENDER",
            if probe == "IMAGE_RENDER_OFF" {
                "0"
            } else {
                "1"
            },
        );
        let output = cmd
            .output()
            .expect("failed to re-exec the test binary for the k8s env probe");
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            output.status.success(),
            "the k8s env probe {probe} failed\n--- child stdout ---\n{stdout}\n--- child stderr ---\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            stdout.contains(&format!("K8S_ENV_PROBE_OK {probe}")),
            "the k8s env probe child did not run for {probe}\n--- child stdout ---\n{stdout}"
        );
    }

    /// Issue #1388: every rendered Job states how its environment was
    /// rendered. A renderable container environment is the pod image and
    /// names no wrapper-fallback; every other environment is explicitly
    /// marked `wrapper-fallback` with the backend kind, so a manifest read
    /// (or `kubectl get job -o yaml`) shows that the pod needs the folded
    /// runtime rather than silently implying image rendering.
    #[test]
    fn environment_render_mode_is_annotated_on_single_and_array_jobs() {
        let env_rule = |name: &str, environment: crate::rule::EnvironmentSpec| Rule {
            name: name.to_string(),
            environment,
            ..Default::default()
        };
        let docker = env_rule(
            "align",
            crate::rule::EnvironmentSpec {
                docker: Some("busybox:1.36".to_string()),
                ..Default::default()
            },
        );
        let conda = env_rule(
            "qc",
            crate::rule::EnvironmentSpec {
                conda: Some("envs/qc.yaml".to_string()),
                ..Default::default()
            },
        );

        let single: Value = serde_json::from_str(
            &generate_k8s_job_manifest(&docker, "cd '/r' && true", &test_config()).unwrap(),
        )
        .unwrap();
        assert_eq!(
            single["metadata"]["annotations"]["oxoflow.io/environment-render"],
            "image"
        );
        assert_eq!(
            single["metadata"]["annotations"]["oxoflow.io/environment-image"],
            "busybox:1.36"
        );

        let array: Value = serde_json::from_str(
            &generate_k8s_array_job_manifest(&docker, "run/cmds", 2, &[], None, &test_config())
                .unwrap(),
        )
        .unwrap();
        assert_eq!(
            array["metadata"]["annotations"]["oxoflow.io/environment-render"], "image",
            "an image-mode array must say so: {array}"
        );

        let fallback: Value = serde_json::from_str(
            &generate_k8s_job_manifest(&conda, "cd '/r' && conda run true", &test_config())
                .unwrap(),
        )
        .unwrap();
        let annotations = &fallback["metadata"]["annotations"];
        assert_eq!(
            annotations["oxoflow.io/environment-render"],
            "wrapper-fallback"
        );
        assert_eq!(annotations["oxoflow.io/environment-kind"], "conda");
        assert!(
            annotations.get("oxoflow.io/environment-image").is_none(),
            "a fallback must not claim an image identity: {fallback}"
        );
        // The historical fallback shape is intact: base image + folded wrapper.
        let container = &fallback["spec"]["template"]["spec"]["containers"][0];
        assert_eq!(container["image"], "ubuntu:latest");
        assert!(
            container["args"][0]
                .as_str()
                .unwrap()
                .contains("conda run true")
        );

        // A `system` rule has no wrapper at all and must not be labelled a
        // fallback (issue #1388 review M2).
        let system = env_rule("report", crate::rule::EnvironmentSpec::default());
        let manifest: Value = serde_json::from_str(
            &generate_k8s_job_manifest(&system, "cd '/r' && true", &test_config()).unwrap(),
        )
        .unwrap();
        let annotations = &manifest["metadata"]["annotations"];
        assert_eq!(annotations["oxoflow.io/environment-render"], "system");
        assert_eq!(annotations["oxoflow.io/environment-kind"], "system");
    }

    /// Issue #1388 review F1: image mode must preserve the replaced
    /// container wrapper's shell contract. The pod starts `/bin/sh` (always
    /// present — busybox/alpine included) and the same bash-if-available shim
    /// selects bash for the rule text; Indexed Jobs exec their per-index FILE
    /// through the file-level shim, so a `set -euo pipefail` prelude works in
    /// an array too.
    #[test]
    fn image_rendered_jobs_run_through_the_bash_shim() {
        let docker = Rule {
            name: "align".to_string(),
            environment: crate::rule::EnvironmentSpec {
                docker: Some("busybox:1.36".to_string()),
                ..Default::default()
            },
            ..Default::default()
        };
        let single: Value = serde_json::from_str(
            &generate_k8s_job_manifest(&docker, "cd '/r' && true", &test_config()).unwrap(),
        )
        .unwrap();
        let container = &single["spec"]["template"]["spec"]["containers"][0];
        assert_eq!(
            container["command"],
            json!(["/bin/sh", "-c"]),
            "a bash-less image must be able to START the container: {single}"
        );
        assert_eq!(
            container["args"],
            json!([
                crate::environment::CONTAINER_BASH_SHIM,
                "sh",
                "cd '/r' && true"
            ]),
            "the rule text must run through the bash-if-available shim: {single}"
        );

        let array: Value = serde_json::from_str(
            &generate_k8s_array_job_manifest(&docker, "run/cmds", 2, &[], None, &test_config())
                .unwrap(),
        )
        .unwrap();
        let container = &array["spec"]["template"]["spec"]["containers"][0];
        assert_eq!(container["command"], json!(["/bin/sh", "-c"]));
        let dispatch = container["args"][0].as_str().unwrap();
        assert!(
            dispatch.contains(crate::environment::CONTAINER_BASH_FILE_SHIM),
            "the per-index file must run with bash when the image has it: {dispatch}"
        );
        assert!(
            dispatch.contains("cmd.${INDEX}.sh"),
            "the dispatch must keep the per-index file path: {dispatch}"
        );

        // Fallback jobs keep the historical `/bin/bash -c` (pre-#1388 shape).
        let conda = Rule {
            name: "qc".to_string(),
            environment: crate::rule::EnvironmentSpec {
                conda: Some("envs/qc.yaml".to_string()),
                ..Default::default()
            },
            ..Default::default()
        };
        let fallback: Value = serde_json::from_str(
            &generate_k8s_job_manifest(&conda, "cd '/r' && conda run true", &test_config())
                .unwrap(),
        )
        .unwrap();
        let container = &fallback["spec"]["template"]["spec"]["containers"][0];
        assert_eq!(container["command"], json!(["/bin/bash", "-c"]));
        assert_eq!(
            container["args"],
            json!(["cd '/r' && conda run true"]),
            "a fallback is not re-wrapped in the shim: {fallback}"
        );
    }

    /// Pure parser behind `OXO_FLOW_K8S_IMAGE_RENDER`: on by default, off for
    /// the documented off spellings, on for truthy values, and a typo warns
    /// and stays ON (never silently changes semantics).
    #[test]
    fn image_rendering_switch_parses_off_truthy_and_junk() {
        assert!(image_rendering_enabled_with(None));
        assert!(image_rendering_enabled_with(Some("")));
        for raw in ["0", "false", "FALSE", "no", "off", " off "] {
            assert!(
                !image_rendering_enabled_with(Some(raw)),
                "{raw:?} must disable image rendering"
            );
        }
        for raw in ["1", "true", "yes", "on", "ON"] {
            assert!(
                image_rendering_enabled_with(Some(raw)),
                "{raw:?} must keep image rendering on"
            );
        }
        assert!(
            image_rendering_enabled_with(Some("tpyo")),
            "an unrecognized value must stay on (the default)"
        );
    }

    /// The probe child started by [`run_k8s_env_probe`]; see the parents for
    /// the injected environment.
    #[test]
    #[ignore]
    fn k8s_env_probe_child() {
        let probe = std::env::var("K8S_ENV_PROBE")
            .expect("the probe child must be started by run_k8s_env_probe");
        // A namespace-qualified image: `mirrored_spec`'s documented scope is
        // `<host>/<path>` and namespace forms — a bare library name
        // (`ubuntu:22.04`, no `/`) passes through untouched, the same
        // semantics the docker/singularity pulls have (bare names resolve
        // through the daemon's own registry-mirror configuration).
        let rule = |name: &str| crate::rule::Rule {
            name: name.to_string(),
            environment: crate::rule::EnvironmentSpec {
                docker: Some("biocontainers/bwa:0.7.17".to_string()),
                ..Default::default()
            },
            ..Default::default()
        };
        // The socket channel serves the WRAPPER FALLBACK (issue #1388): a
        // conda environment cannot be rendered as a pod image, so its pod
        // still executes the folded local wrapper and may need the daemon.
        let fallback_rule = |name: &str| crate::rule::Rule {
            name: name.to_string(),
            environment: crate::rule::EnvironmentSpec {
                conda: Some("envs/align.yaml".to_string()),
                ..Default::default()
            },
            ..Default::default()
        };
        let manifest = |r: &crate::rule::Rule| {
            serde_json::from_str::<Value>(
                &generate_k8s_job_manifest(r, "cd '/abs/run' && true", &k8s_test_config())
                    .expect("probe manifest must render"),
            )
            .expect("probe manifest must be valid json")
        };
        match probe.as_str() {
            // The bare name is a docker.io namespace and follows the mapping
            // the parent injected; with only a quay.io entry the docker.io
            // namespace passes through untouched.
            "MIRROR_DOCKER" => {
                let val = manifest(&rule("pull"));
                assert_eq!(
                    val["spec"]["template"]["spec"]["containers"][0]["image"],
                    "mirror.example.com/biocontainers/bwa:0.7.17",
                    "the docker.io mapping must rewrite the manifest image: {val}"
                );
                // The manifest annotation keeps the WORKFLOW spec: mirror
                // rewriting is transport-only, so the recorded identity does
                // not change on a mirrored box (issue #1388).
                assert_eq!(
                    val["metadata"]["annotations"]["oxoflow.io/environment-image"],
                    "biocontainers/bwa:0.7.17",
                    "the identity annotation must stay mirror-neutral: {val}"
                );
                // Image mode means no local `docker run` in the pod body.
                assert_eq!(
                    val["metadata"]["annotations"]["oxoflow.io/environment-render"], "image",
                    "{val}"
                );
            }
            "MIRROR_OTHER" => {
                let val = manifest(&rule("pull"));
                assert_eq!(
                    val["spec"]["template"]["spec"]["containers"][0]["image"],
                    "biocontainers/bwa:0.7.17",
                    "a namespace not in the map must pass through: {val}"
                );
            }
            "IMAGE_RENDER_OFF" => {
                assert!(
                    !image_rendering_enabled(),
                    "the child must see the injected opt-out"
                );
                let val = manifest(&rule("wrap"));
                let container = &val["spec"]["template"]["spec"]["containers"][0];
                assert_eq!(
                    container["image"], "biocontainers/bwa:0.7.17",
                    "the fallback keeps the rule's base image: {val}"
                );
                assert_eq!(container["command"], json!(["/bin/bash", "-c"]));
                assert_eq!(
                    val["metadata"]["annotations"]["oxoflow.io/environment-render"],
                    "wrapper-fallback",
                    "an opted-out renderable environment is a wrapper fallback: {val}"
                );
                assert!(
                    val["metadata"]["annotations"]
                        .get("oxoflow.io/environment-image")
                        .is_none(),
                    "no image identity may be claimed when rendering is off: {val}"
                );
            }
            on_off @ ("SOCKET_ON" | "SOCKET_OFF") => {
                let val = manifest(&fallback_rule("wrap"));
                let pod_spec = &val["spec"]["template"]["spec"];
                let socket_volume = pod_spec["volumes"].as_array().and_then(|v| {
                    v.iter()
                        .find(|v| v["name"] == "oxoflow-docker-sock")
                        .cloned()
                });
                let container = &pod_spec["containers"][0];
                if on_off == "SOCKET_ON" {
                    let socket_volume = socket_volume.expect("socket volume must be present");
                    assert_eq!(socket_volume["hostPath"]["path"], "/var/run/docker.sock");
                    assert_eq!(socket_volume["hostPath"]["type"], "Socket");
                    assert!(
                        container["volumeMounts"]
                            .as_array()
                            .unwrap()
                            .iter()
                            .any(|m| m["name"] == "oxoflow-docker-sock"
                                && m["mountPath"] == "/var/run/docker.sock"),
                        "socket mount missing: {val}"
                    );
                    assert!(
                        container["env"]
                            .as_array()
                            .unwrap()
                            .iter()
                            .any(|e| e["name"] == "DOCKER_HOST"
                                && e["value"] == "unix:///var/run/docker.sock"),
                        "DOCKER_HOST must point at the mounted socket: {val}"
                    );
                    // Issue #1388: an IMAGE-RENDERED rule needs no daemon —
                    // the opt-in must not mount a host-root-equivalent socket
                    // into a pod that cannot use it.
                    let image_val = manifest(&rule("image"));
                    let image_pod = &image_val["spec"]["template"]["spec"];
                    assert!(
                        !image_pod["volumes"]
                            .as_array()
                            .is_some_and(|v| v.iter().any(|v| v["name"] == "oxoflow-docker-sock")),
                        "an image-rendered pod must not receive the daemon socket: {image_val}"
                    );
                    assert!(
                        image_pod["containers"][0].get("env").is_none(),
                        "an image-rendered pod must not export DOCKER_HOST: {image_val}"
                    );
                } else {
                    // Off: neither the volume nor the mount may exist — the
                    // unrecognized value must not enable the channel.
                    assert!(
                        socket_volume.is_none(),
                        "the unrecognized value must not mount the socket: {val}"
                    );
                    assert!(
                        !container
                            .get("env")
                            .map(|e| e
                                .as_array()
                                .unwrap()
                                .iter()
                                .any(|e| e["name"] == "DOCKER_HOST"))
                            .unwrap_or(false),
                        "the off state must not export DOCKER_HOST: {val}"
                    );
                }
            }
            other => panic!("unknown k8s env probe {other}"),
        }
        println!("K8S_ENV_PROBE_OK {probe}");
    }

    /// `[cluster.volume]` mode (issue #1387): the pod mounts the SHARED
    /// volume at mount_root, the rendered command's folded `cd` and every
    /// workdir-anchored path are rewritten onto the mount, and NO
    /// submit-host hostPath is emitted. NFS shape.
    #[test]
    fn cluster_volume_nfs_replaces_hostpath_and_rewrites_the_command() {
        let rule = Rule {
            name: "wrap".into(),
            ..Default::default()
        };
        let mut config = k8s_test_config();
        config.volume = Some(crate::config::ClusterVolume {
            kind: "nfs".into(),
            server: Some("10.0.0.9".into()),
            path: Some("/exports/oxoflow".into()),
            volume_root: Some("/abs".into()),
            ..Default::default()
        });
        let cmd = "cd '/abs/run' && docker run --rm -v '/abs/run/data':'/abs/run/data' 'img' sh -c 'true > out.txt'";
        let val: Value =
            serde_json::from_str(&generate_k8s_job_manifest(&rule, cmd, &config).unwrap()).unwrap();
        let pod_spec = &val["spec"]["template"]["spec"];
        let container = &pod_spec["containers"][0];
        assert_eq!(
            container["volumeMounts"][0],
            json!({"name":"oxoflow-shared","mountPath":"/oxoflow"}),
            "the shared volume must mount at the default mount_root"
        );
        assert_eq!(
            pod_spec["volumes"][0]["nfs"],
            json!({"server":"10.0.0.9","path":"/exports/oxoflow"}),
            "NFS source must render with server + path"
        );
        assert!(
            !serde_json::to_string(&pod_spec["volumes"])
                .unwrap()
                .contains("hostPath"),
            "volume mode must not leak a submit-host hostPath: {pod_spec}"
        );
        let rendered = container["args"][0].as_str().unwrap();
        assert!(
            rendered.contains("cd '/oxoflow/run'"),
            "folded cd must be rewritten to mount_root/<relative>: {rendered}"
        );
        assert!(
            rendered.contains("-v '/oxoflow/run/data':'/oxoflow/run/data'"),
            "workdir-anchored bind paths must follow the mount: {rendered}"
        );
        assert!(
            !rendered.contains("/abs/run"),
            "no submit-host path may survive: {rendered}"
        );
    }

    /// PVC shape, custom mount_root, and the array dispatch path rewrite:
    /// the command directory under the run workdir maps onto the mount so
    /// indexed elements find their `cmd.$N.sh` inside the shared volume.
    #[test]
    fn cluster_volume_pvc_rewrites_array_dispatch_paths() {
        let rule = Rule {
            name: "scatter".into(),
            ..Default::default()
        };
        let mut config = k8s_test_config();
        config.volume = Some(crate::config::ClusterVolume {
            kind: "pvc".into(),
            claim_name: Some("oxoflow-work".into()),
            volume_root: Some("/abs".into()),
            mount_root: Some("/mnt/work".into()),
            ..Default::default()
        });
        let val: Value = serde_json::from_str(
            &generate_k8s_array_job_manifest(
                &rule,
                "/abs/run/.oxo-flow/runs/r1/jobs/scatter/chunk-1",
                2,
                &[],
                Some("/abs/run"),
                &config,
            )
            .unwrap(),
        )
        .unwrap();
        let pod_spec = &val["spec"]["template"]["spec"];
        let container = &pod_spec["containers"][0];
        assert!(
            container["args"][0]
                .as_str()
                .unwrap()
                .contains("sh \"/mnt/work/run/.oxo-flow/runs/r1/jobs/scatter/chunk-1/cmd."),
            "the dispatch path must map onto the mount root: {}",
            container["args"][0]
        );
        assert_eq!(
            pod_spec["volumes"][0]["persistentVolumeClaim"],
            json!({"claimName":"oxoflow-work"})
        );
        assert_eq!(container["volumeMounts"][0]["mountPath"], "/mnt/work");
        assert!(
            !serde_json::to_string(&pod_spec["volumes"])
                .unwrap()
                .contains("hostPath")
        );
    }

    /// Regression (issue #1226): two chunks of the SAME template must not
    /// render the same `metadata.name`, or the second `kubectl create` fails
    /// the run on a name collision. The suffix is derived from the per-chunk
    /// job dir, so sibling chunks differ, the same chunk re-renders stably
    /// (idempotent), and the whole name stays within the RFC 1123 cap.
    #[test]
    fn test_k8s_array_job_name_is_unique_per_chunk() {
        let rule = Rule {
            name: "scatter_qc".into(),
            ..Default::default()
        };
        let chunk1 = generate_k8s_array_job_manifest(
            &rule,
            "run/jobs/scatter_qc/chunk-1",
            3,
            &[],
            None,
            &test_config(),
        )
        .expect("chunk 1 manifest must render");
        let chunk2 = generate_k8s_array_job_manifest(
            &rule,
            "run/jobs/scatter_qc/chunk-2",
            3,
            &[],
            None,
            &test_config(),
        )
        .expect("chunk 2 manifest must render");
        let chunk1_again = generate_k8s_array_job_manifest(
            &rule,
            "run/jobs/scatter_qc/chunk-1",
            3,
            &[],
            None,
            &test_config(),
        )
        .expect("chunk 1 re-render must succeed");

        let name = |m: &str| -> String {
            let v: Value = serde_json::from_str(m).expect("manifest must be valid json");
            v["metadata"]["name"]
                .as_str()
                .expect("name must be a string")
                .to_string()
        };
        let n1 = name(&chunk1);
        let n2 = name(&chunk2);
        let n1_again = name(&chunk1_again);

        assert_ne!(n1, n2, "sibling chunks must not share a job name");
        assert_eq!(n1, n1_again, "the same chunk must re-render identically");
        // RFC 1123 label cap.
        assert!(n1.len() <= 63, "job name {n1} exceeds the 63-char cap");
        // A very long rule name must be truncated, never overflow the cap.
        let long_rule = Rule {
            name: "x".repeat(200),
            ..Default::default()
        };
        let long_name = name(
            &generate_k8s_array_job_manifest(
                &long_rule,
                "run/jobs/long/chunk-1",
                2,
                &[],
                None,
                &test_config(),
            )
            .expect("long-name manifest must render"),
        );
        assert!(
            long_name.len() <= 63,
            "truncated job name {long_name} still exceeds the cap"
        );
    }

    /// Sentinel regression (ledger RB-12): once the plan routes a
    /// `sensitive = true` value into an `OXO_FLOW_SENSITIVE_<n>` reference,
    /// the manifest carries a `secretKeyRef`, never the literal.
    #[test]
    fn test_k8s_manifest_projects_sensitive_values_from_a_secret() {
        let sentinel = "SENTINEL_SECRET_VALUE_D4";
        let (routed, env) = crate::executor::process::route_sensitive_values_through_env(
            &format!("tool --token {sentinel} --out x"),
            &[sentinel.to_string()],
        );
        assert_eq!(
            env.get("OXO_FLOW_SENSITIVE_0").map(String::as_str),
            Some(sentinel)
        );

        let rule = Rule {
            name: "call_variants".into(),
            ..Default::default()
        };
        let manifest_str =
            generate_k8s_job_manifest(&rule, &routed, &test_config()).expect("manifest");
        assert!(
            !manifest_str.contains(sentinel),
            "sentinel leaked into the k8s manifest: {manifest_str}"
        );
        let val: Value = serde_json::from_str(&manifest_str).unwrap();
        let env_entry = &val["spec"]["template"]["spec"]["containers"][0]["env"][0];
        assert_eq!(env_entry["name"], "OXO_FLOW_SENSITIVE_0");
        let key_ref = &env_entry["valueFrom"]["secretKeyRef"];
        assert_eq!(key_ref["name"], K8S_SENSITIVE_SECRET);
        assert_eq!(key_ref["key"], "OXO_FLOW_SENSITIVE_0");
    }

    /// Issue #1171: a submit-host ambient credential must never reach a
    /// Kubernetes job body. The manifest is built from the rule and the
    /// rendered command only; the pod's environment is the image `ENV` plus
    /// what this generator writes, and it writes no ambient state. The probe
    /// runs in a child process that carries sentinel credentials (the crate
    /// forbids `std::env::set_var`), so a future change that forwards the
    /// submit host's environment or inlines its values fails here.
    #[test]
    fn kubernetes_job_body_carries_no_submit_host_environment() {
        crate::backend::assert_cloud_body_ignores_submit_host_env("kubernetes");
    }

    /// Indexed jobs dispatch to per-index command files, so the dispatch line
    /// carries no reference — the driver passes the union in. The env block is
    /// still attached, and ordinary (reference-free) jobs stay untouched.
    #[test]
    fn test_k8s_array_manifest_attaches_the_routed_secret_refs() {
        let rule = Rule {
            name: "scatter_qc".into(),
            ..Default::default()
        };
        let refs = vec!["OXO_FLOW_SENSITIVE_0".to_string()];
        let manifest_str =
            generate_k8s_array_job_manifest(&rule, "run/cmds", 5, &refs, None, &test_config())
                .unwrap();
        let val: Value = serde_json::from_str(&manifest_str).unwrap();
        let env_entry = &val["spec"]["template"]["spec"]["containers"][0]["env"][0];
        assert_eq!(env_entry["name"], "OXO_FLOW_SENSITIVE_0");

        let plain =
            generate_k8s_array_job_manifest(&rule, "run/cmds", 5, &[], None, &test_config())
                .unwrap();
        assert!(
            !plain.contains("\"env\""),
            "a reference-free job must stay byte-identical: {plain}"
        );
    }

    /// Ledger RB-06: the Indexed Job status fields the driver's per-element
    /// poller reads must parse into the completion-index set they name.
    #[test]
    fn parse_index_list_reads_the_indexed_job_status_format() {
        assert_eq!(
            parse_index_list("0-2,4"),
            BTreeSet::from([0, 1, 2, 4]),
            "ranges are inclusive and comma-separated"
        );
        assert_eq!(parse_index_list("3"), BTreeSet::from([3]));
        assert_eq!(parse_index_list(""), BTreeSet::new());
        // Defensive: junk never fabricates an index.
        assert_eq!(parse_index_list("4-2"), BTreeSet::new());
        assert_eq!(parse_index_list("x,1,,2-y"), BTreeSet::from([1]));
    }

    /// Ledger RB-06: per-element verdicts stay independent, and missing
    /// failedIndexes data is Unknown — not success, and not copied onto
    /// every element.
    #[test]
    fn indexed_element_status_is_per_element_and_never_fabricates() {
        let completed = parse_index_list("0,2");
        let failed = parse_index_list("1");
        for (index, expected) in [
            (0, BackendJobStatus::Completed),
            (1, BackendJobStatus::Failed),
            (2, BackendJobStatus::Completed),
            // Index 3 is neither finished nor failed: still live.
            (3, BackendJobStatus::Running),
        ] {
            assert_eq!(
                indexed_element_status(&completed, &failed, 1, index, None),
                expected,
                "index {index} must have its own verdict"
            );
        }
        // Kubernetes < 1.31 has status.failed but no failedIndexes: the
        // Job is terminal-failed and the failing element is unknowable.
        // It must be Unknown, never success and never assumed to be index 0.
        let unknown = indexed_element_status(&BTreeSet::new(), &BTreeSet::new(), 2, 1, None);
        assert_eq!(
            unknown,
            BackendJobStatus::Unknown,
            "missing failedIndexes must not pin the failure on a guessed element"
        );
    }

    /// backoffLimit: 0 makes a terminal-Failed Indexed Job final: the
    /// controller never creates the remaining pods (`parallelism` caps
    /// in-flight elements), so indexes recorded in NEITHER status list
    /// would previously report `Running` forever — resetting the driver's
    /// blind-settlement guard and polling a dead Job with no wall-clock
    /// budget. A terminal-Failed Job settles the unrecorded index as
    /// Failed; a Complete Job with an unrecorded index is contradictory
    /// wire data, so it settles as Unknown (never fabricated success).
    #[test]
    fn unrecorded_index_settles_when_the_job_is_terminal() {
        let completed = parse_index_list("0");
        let failed = parse_index_list("1");
        // Indexes 2..63 never got a pod (parallelism capped at 2): the
        // recorded sets prove nothing about them, the Job does.
        for index in [2, 63] {
            assert_eq!(
                indexed_element_status(
                    &completed,
                    &failed,
                    1,
                    index,
                    Some(IndexedJobTerminal::Failed)
                ),
                BackendJobStatus::Failed,
                "an unrecorded index on a terminal-Failed Job can never run"
            );
        }
        // The recorded verdicts are untouched by the terminal phase.
        assert_eq!(
            indexed_element_status(&completed, &failed, 1, 0, Some(IndexedJobTerminal::Failed)),
            BackendJobStatus::Completed
        );
        assert_eq!(
            indexed_element_status(&completed, &failed, 1, 1, Some(IndexedJobTerminal::Failed)),
            BackendJobStatus::Failed
        );
        // A live Job keeps unrecorded indexes live — the fix must not
        // settle anything on a Job that is still running.
        assert_eq!(
            indexed_element_status(&completed, &failed, 1, 2, None),
            BackendJobStatus::Running
        );
        // Complete-but-unrecorded is contradictory wire data: Unknown, so
        // the driver's terminal-unknown guard can settle it, never a
        // fabricated success.
        assert_eq!(
            indexed_element_status(
                &completed,
                &failed,
                1,
                2,
                Some(IndexedJobTerminal::Complete)
            ),
            BackendJobStatus::Unknown
        );
        // The blind state (no failedIndexes field) keeps its documented
        // Unknown contract even on a terminal-Failed Job.
        assert_eq!(
            indexed_element_status(
                &BTreeSet::new(),
                &BTreeSet::new(),
                2,
                1,
                Some(IndexedJobTerminal::Failed)
            ),
            BackendJobStatus::Unknown
        );
    }
}
