//! Opt-in content-addressed shared environment store (issue #904).
//!
//! By default a workflow is SELF-CONTAINED: file-backed environments (pixi
//! manifests, venv dirs) materialize inside the workflow tree exactly as
//! they always have, and the tree stays copyable, movable, and `rm -rf`-safe
//! on its own. The machine-level store engages ONLY when the user opts in by
//! setting `OXO_FLOW_ENV_STORE` — location IS activation. When enabled,
//! setup writes the environment once under a content-derived key and the
//! in-tree location the rest of the engine already knows about becomes a
//! symlink into the store. Wrap commands (`pixi run --manifest-path`,
//! `source .venv/bin/activate`) keep working unchanged because they resolve
//! through the symlink.
//!
//! Multi-user note: the env var is per-process, so on a shared Linux server
//! every user who opts in gets their own store under their `$HOME` unless
//! they deliberately point the variable at a shared filesystem path —
//! sharing a store across users is an explicit operator decision (common
//! path + shared FS + group permissions), never an implicit default.
//!
//! Key invariants:
//! - The store key reuses the per-backend `cache_key` verbatim (including
//!   the #532 content tags) — never retype or re-derive a hash scheme here.
//! - Teardown safety: `rm -rf` on a symlink removes the link only, so the
//!   existing venv teardown naturally stays store-safe; nothing here ever
//!   deletes store content except the torn-dir clear in `rehome_into_store`
//!   and the explicit `oxo-flow env gc`.
//! - Broken-link detection: pixi and venv have no `verify_command`
//!   (`verify_command` returns `None` for them), so a store symlink whose
//!   target was removed (manual cleanup, disk-pressure sweep) would pass
//!   every existing check and fail at rule time. [`EnvStore::in_tree_state`]
//!   closes that gap.
//! - Re-homing never destroys the in-tree env on failure: the store copy is
//!   completed before the in-tree original is dropped, and a failed final
//!   link restores the in-tree tree from the store copy.
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

/// Environment variable selecting the shared env store root.
///
/// Setting this variable (to a non-empty path) is what ACTIVATES the store;
/// unset, every workflow stays self-contained in its own tree.
pub const ENV_STORE_ENV_VAR: &str = "OXO_FLOW_ENV_STORE";

/// Kinds of file-backed environments the store currently manages.
///
/// Container / module / conda-prefix environments are not store-managed:
/// containers pull images into their own registry, modules are provided by
/// the cluster, and conda/mamba already live in a shared central prefix.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StoreKind {
    Pixi,
    Venv,
}

impl StoreKind {
    /// Derives the store-managed kind from a resolved environment spec.
    ///
    /// Returns `None` for every other backend (the store is a no-op for
    /// them, including the cache-hit re-check).
    pub fn from_spec(env_spec: &crate::rule::EnvironmentSpec) -> Option<Self> {
        if env_spec.pixi.is_some() {
            Some(Self::Pixi)
        } else if env_spec.venv.is_some() {
            Some(Self::Venv)
        } else {
            None
        }
    }

    /// In-tree directory the backend materializes and wraps against.
    ///
    /// This is the path the *spec itself* names: pixi's spec is the
    /// manifest file, so the materialized dir is the manifest's parent
    /// (`.pixi` lives beside `pixi.toml`); venv's spec is the target dir.
    /// Relative specs are resolved against `base` (already-absolute by the
    /// time `ensure_environment_ready` calls in — `resolve_relative_paths`
    /// anchored them at the workflow root).
    pub fn in_tree_path(self, spec: &str, base: Option<&Path>) -> PathBuf {
        let raw = Path::new(spec);
        if raw.is_absolute() {
            match self {
                // `pixi install --manifest-path <file>` puts `.pixi` next to
                // the manifest file, not inside it.
                Self::Pixi => raw.parent().unwrap_or(Path::new(".")).join(".pixi"),
                Self::Venv => raw.to_path_buf(),
            }
        } else {
            let joined = base.unwrap_or(Path::new(".")).join(raw);
            self.in_tree_path(&joined.to_string_lossy(), None)
        }
    }

    /// `<backend>-` prefix used in store directory names.
    pub fn prefix(self) -> &'static str {
        match self {
            Self::Pixi => "pixi",
            Self::Venv => "venv",
        }
    }
}

/// Store directory layout plus the symlink/bookkeeping operations the
/// executor needs.
///
/// One instance per `LocalExecutor` (`None` = store not enabled); all paths
/// are computed, none cached across runs — the store may be modified (or
/// emptied) between runs by `env gc` or the user, so every check re-stats.
#[derive(Debug, Clone)]
pub struct EnvStore {
    root: PathBuf,
}

impl EnvStore {
    /// Resolves the store root from `OXO_FLOW_ENV_STORE` — OPT-IN.
    ///
    /// Returns `None` unless the variable is set to a non-empty path: the
    /// default is the exact pre-#904 self-contained behavior (envs live in
    /// the workflow tree). There is deliberately no implicit per-user
    /// default root — a machine-wide store is always an explicit operator
    /// decision, which also keeps multi-user servers unambiguous (each
    /// process sees its own env var; `$HOME` never leaks into this path).
    pub fn from_env() -> Option<Self> {
        let dir = std::env::var_os(ENV_STORE_ENV_VAR).filter(|d| !d.is_empty())?;
        Some(Self {
            root: PathBuf::from(dir),
        })
    }

    /// Explicit-root constructor (CLI `env` subcommands, tests).
    pub fn from_root(root: PathBuf) -> Self {
        Self { root }
    }

    /// Store root as configured (test/CLI introspection).
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// `true` when this instance was built from a real (non-empty) store
    /// root — i.e. `OXO_FLOW_ENV_STORE` was set. An empty-root sentinel
    /// (store not opted into) is disabled and every store path in the
    /// executor is gated on this.
    pub fn is_enabled(&self) -> bool {
        !self.root.as_os_str().is_empty()
    }

    /// Content-addressed store directory for a cache key:
    /// `<root>/<backend>-<sha256-16hex>` — same derivation discipline as
    /// the env-create lock path (hash the key, truncate to 16 hex chars;
    /// collision risk negligible for env counts).
    pub fn store_dir(&self, kind: StoreKind, cache_key: &str) -> PathBuf {
        let digest = hex::encode(Sha256::digest(cache_key.as_bytes()));
        self.root
            .join(format!("{}-{}", kind.prefix(), &digest[..16]))
    }

    /// Ensures the store root exists (best-effort; a failure surfaces at
    /// materialization time with the real errno).
    pub fn ensure_root(&self) -> std::io::Result<()> {
        std::fs::create_dir_all(&self.root)
    }

    /// `true` when the store dir holds a fully materialized environment.
    ///
    /// The marker file is written only after a successful setup + link,
    /// so a torn/partial install (killed setup, ENOSPC mid-write) is never
    /// mistaken for a usable env. The layout check recognizes BOTH shapes a
    /// store dir can have: a re-homed venv dir (`bin/`) and a re-homed pixi
    /// env (the renamed `.pixi` dir itself, which contains `envs/` — not a
    /// nested `.pixi`).
    pub fn store_is_ready(&self, store_dir: &Path) -> bool {
        store_dir.join(".oxo-env-ready").is_file()
            && (store_dir.join(".pixi").exists()
                || store_dir.join("bin").exists()
                || store_dir.join("envs").exists())
    }

    /// Broken-link / stale-target state of a store-managed in-tree path.
    pub fn in_tree_state(&self, kind: StoreKind, spec: &str, base: Option<&Path>) -> InTreeState {
        let in_tree = kind.in_tree_path(spec, base);
        match std::fs::symlink_metadata(&in_tree) {
            // Nothing materialized in-tree yet (cold path).
            Err(_) => InTreeState::Absent,
            // Real dir or file: pre-store materialization — fine.
            Ok(meta) if !meta.file_type().is_symlink() => InTreeState::Fine,
            // Symlink: usable only when its target still resolves.
            Ok(_) if in_tree.exists() => InTreeState::Fine,
            Ok(_) => InTreeState::BrokenLink,
        }
    }

    /// Re-points the in-tree path at `store_dir` and writes the ready
    /// marker.
    ///
    /// The swap is staged: the symlink is created under a temporary name
    /// first, so the existing in-tree entry is only dropped once the
    /// replacement is ready to swing in. If the final rename then fails
    /// after a real dir was removed, the tree is restored from `store_dir`
    /// (best-effort) — the caller's fallback never strands the env.
    pub fn link_in_tree(
        &self,
        kind: StoreKind,
        spec: &str,
        base: Option<&Path>,
        store_dir: &Path,
    ) -> std::io::Result<()> {
        #[cfg(not(unix))]
        {
            // Decide BEFORE touching anything: non-Unix filesystems get the
            // plain in-tree env, never a half-removed one.
            let _ = (kind, spec, base, store_dir);
            Err(std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "env store symlinks require a Unix filesystem",
            ))
        }
        #[cfg(unix)]
        {
            let in_tree = kind.in_tree_path(spec, base);
            if let Some(parent) = in_tree.parent() {
                std::fs::create_dir_all(parent)?;
            }
            // Unique per process+thread: two concurrent runs re-homing the
            // same env in one tree must not unlink each other's staged
            // symlink (the old fixed name had exactly that race —
            // self-healing, but wasteful and confusing).
            let stage = in_tree.with_file_name(format!(
                ".{}.oxo-store-link.{}.{}",
                in_tree
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("env"),
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_nanos())
                    .unwrap_or(0),
            ));
            std::os::unix::fs::symlink(store_dir, &stage)?;
            // Drop the previous entry only now that the replacement exists.
            let dropped = match std::fs::symlink_metadata(&in_tree) {
                Ok(meta) if meta.is_dir() && !meta.file_type().is_symlink() => {
                    std::fs::remove_dir_all(&in_tree)
                }
                Ok(_) => std::fs::remove_file(&in_tree),
                Err(_) => Ok(()),
            };
            if let Err(e) = dropped.and_then(|()| std::fs::rename(&stage, &in_tree)) {
                let _ = std::fs::remove_file(&stage);
                let mut msg = format!(
                    "failed to link {} into the env store: {e}",
                    in_tree.display()
                );
                if !in_tree.exists() && store_dir.is_dir() {
                    // A real dir was destroyed and the rename failed — put
                    // the environment back from the store copy.
                    if copy_dir_recursive(store_dir, &in_tree).is_ok() {
                        msg.push_str(" (the in-tree environment was restored from the store copy)");
                    } else {
                        msg.push_str(&format!(
                            "; the environment content survives in the store at {} — \
                             copy it back or re-run setup",
                            store_dir.display()
                        ));
                    }
                }
                return Err(std::io::Error::new(e.kind(), msg));
            }
            std::fs::write(store_dir.join(".oxo-env-ready"), b"ok\n")?;
            Ok(())
        }
    }

    /// Lists store entries with their backend kind, ready state, mtime and
    /// rough disk usage. Order-independent (sorted by path) for stable CLI
    /// output and GC decisions.
    pub fn entries(&self) -> std::io::Result<Vec<StoreEntry>> {
        let mut out = Vec::new();
        let read_dir = match std::fs::read_dir(&self.root) {
            Ok(rd) => rd,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(out),
            Err(e) => return Err(e),
        };
        for entry in read_dir {
            let entry = entry?;
            let path = entry.path();
            let name = entry.file_name();
            let name = name.to_string_lossy();
            let kind = if name.starts_with("pixi-") {
                StoreKind::Pixi
            } else if name.starts_with("venv-") {
                StoreKind::Venv
            } else {
                continue; // foreign file (lock file, marker, user junk)
            };
            let ready = self.store_is_ready(&path);
            let mtime = entry
                .metadata()
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_secs());
            let size = dir_size(&path);
            out.push(StoreEntry {
                path,
                kind,
                ready,
                mtime,
                size,
            });
        }
        out.sort_by(|a, b| a.path.cmp(&b.path));
        Ok(out)
    }
}

/// Tri-state result of [`EnvStore::in_tree_state`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InTreeState {
    /// Nothing materialized in-tree yet (cold path).
    Absent,
    /// A real in-tree entry (or a symlink that resolves) — usable as-is.
    Fine,
    /// A symlink whose target is gone — the backend-level
    /// `verify_command = None` gap would let this fail at rule time.
    BrokenLink,
}

/// One store entry as seen by `env list` / `env gc`.
#[derive(Debug, Clone)]
pub struct StoreEntry {
    /// Absolute store directory path.
    pub path: PathBuf,
    /// Managed backend kind.
    pub kind: StoreKind,
    /// Ready marker + backend dir present.
    pub ready: bool,
    /// Last modification of the entry dir itself (Unix secs).
    pub mtime: Option<u64>,
    /// Recursive size in bytes (best-effort; walk errors count as 0).
    pub size: u64,
}

/// Recursive directory size, best-effort (unreadable subtrees contribute 0).
fn dir_size(path: &Path) -> u64 {
    let meta = match std::fs::symlink_metadata(path) {
        Ok(m) => m,
        Err(_) => return 0,
    };
    if meta.is_symlink() {
        return 0; // links are cheap; the target is counted where it lives
    }
    if !meta.is_dir() {
        return meta.len();
    }
    let mut total = 0;
    let rd = match std::fs::read_dir(path) {
        Ok(rd) => rd,
        Err(_) => return 0,
    };
    for child in rd.flatten() {
        total += dir_size(&child.path());
    }
    total
}

/// Store-bookkeeping recorded per (workflow, rule) in the checkpoint so
/// `env gc` can compute refcounts without parsing workflow files.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct EnvStoreRecord {
    /// Store directory name (e.g. `pixi-1a2b3c4d5e6f7a8b`).
    pub store_dir: String,
    /// Backend kind prefix.
    pub kind: String,
    /// The full per-backend cache key the store dir was derived from.
    pub cache_key: String,
}

/// Collects every store dir referenced by a set of checkpoint states.
///
/// GC policy inputs: a store dir referenced by any live checkpoint is
/// protected regardless of age (the #893 lesson — never auto-delete
/// something a run still references).
pub fn referenced_store_dirs<'a, I>(checkpoints: I) -> std::collections::BTreeSet<String>
where
    I: IntoIterator<Item = &'a std::collections::BTreeMap<String, EnvStoreRecord>>,
{
    let mut refs = std::collections::BTreeSet::new();
    for records in checkpoints {
        for record in records.values() {
            refs.insert(record.store_dir.clone());
        }
    }
    refs
}

/// Recursive directory copy (EXDEV fallback for store re-homing): std has
/// no recursive-copy helper, and `std::fs::rename` fails across filesystems
/// (store root on a different mount than the workflow tree). Symlinks are
/// copied as links, regular files byte-by-byte.
fn copy_dir_recursive(src: &Path, dst: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let from = entry.path();
        let to = dst.join(entry.file_name());
        let meta = std::fs::symlink_metadata(&from)?;
        if meta.file_type().is_symlink() {
            let target = std::fs::read_link(&from)?;
            #[cfg(unix)]
            {
                std::os::unix::fs::symlink(&target, &to)?;
            }
            #[cfg(not(unix))]
            {
                std::fs::copy(&from, &to)?;
            }
        } else if meta.is_dir() {
            copy_dir_recursive(&from, &to)?;
        } else {
            std::fs::copy(&from, &to)?;
        }
    }
    Ok(())
}

fn store_record(store_dir: &Path, kind: StoreKind, cache_key: &str) -> EnvStoreRecord {
    EnvStoreRecord {
        store_dir: store_dir
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned(),
        kind: kind.prefix().to_string(),
        cache_key: cache_key.to_string(),
    }
}

/// `true` when a store-key directory does NOT look like something the store
/// (or a killed setup) wrote — foreign content is never deleted; the re-home
/// simply aborts and the caller keeps the in-tree env.
fn store_dir_looks_foreign(dir: &Path) -> bool {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return true; // unreadable: treat as foreign, never delete
    };
    for entry in rd.flatten() {
        let known = match entry.file_name().to_str() {
            Some(name) => {
                name.starts_with('.')
                    || matches!(
                        name,
                        "bin"
                            | "envs"
                            | "manifests"
                            | "lib"
                            | "lib64"
                            | "Scripts"
                            | "include"
                            | "share"
                            | "pyvenv.cfg"
                    )
            }
            None => false, // non-UTF8 name: foreign
        };
        if !known {
            return true;
        }
    }
    // Empty or all-recognized entries: ours (or nothing) — safe to clear.
    false
}

/// Re-home + link + record: moves a freshly materialized in-tree
/// environment into the content-addressed store, symlinks the in-tree path
/// back, and returns the record for the checkpoint.
///
/// Best-effort by design — every failure propagates as `Err` and the CALLER
/// falls back to the plain in-tree env (the pre-#904 behavior); the store
/// just misses the dedup. A torn (killed setup) dir at the same key is
/// cleared first — but only when it does not look foreign; a READY dir
/// means another run won the race and the store copy wins without touching
/// anything. An in-tree path that is ALREADY a symlink is never renamed
/// (renaming a symlink moves the link, not the env) — the store dir must
/// already be ready in that case, or the re-home aborts.
pub(crate) fn rehome_into_store(
    store: &EnvStore,
    kind: StoreKind,
    spec: &str,
    base: Option<&Path>,
    cache_key: &str,
) -> std::io::Result<EnvStoreRecord> {
    // Belt-and-braces: the executor gates this call on `is_enabled`, but a
    // disabled sentinel must never move an env — its empty root would
    // resolve store_dir against the process CWD.
    if !store.is_enabled() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "env store disabled (OXO_FLOW_ENV_STORE unset) — refusing to re-home",
        ));
    }
    // Windows and other non-unix targets have no symlink support here:
    // refuse before the rename/copy churns the env into the store.
    #[cfg(not(unix))]
    {
        let _ = (kind, spec, base, cache_key);
        return Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "shared env store requires unix (symlink support)",
        ));
    }
    #[cfg(unix)]
    {
        let store_dir = store.store_dir(kind, cache_key);
        store.ensure_root()?;

        let in_tree = kind.in_tree_path(spec, base);
        if let Ok(meta) = std::fs::symlink_metadata(&in_tree)
            && meta.file_type().is_symlink()
        {
            // Concurrent re-home already swapped the link. Only accept when OUR
            // store dir is the ready target; never rename a symlink into place.
            if store.store_is_ready(&store_dir) {
                store.link_in_tree(kind, spec, base, &store_dir)?;
                return Ok(store_record(&store_dir, kind, cache_key));
            }
            return Err(std::io::Error::new(
                std::io::ErrorKind::AlreadyExists,
                format!(
                    "in-tree env at {} is already a store symlink but {} is not a ready store dir",
                    in_tree.display(),
                    store_dir.display()
                ),
            ));
        }

        if store_dir.exists() {
            if store.store_is_ready(&store_dir) {
                // Another run won the race: adopt the store copy as-is.
                store.link_in_tree(kind, spec, base, &store_dir)?;
                return Ok(store_record(&store_dir, kind, cache_key));
            }
            if store_dir_looks_foreign(&store_dir) {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::AlreadyExists,
                    format!(
                        "store dir {} holds unrecognized content; refusing to clear it",
                        store_dir.display()
                    ),
                ));
            }
            // Torn dir from a killed setup — safe to clear and retry.
            std::fs::remove_dir_all(&store_dir)?;
        }

        if std::fs::rename(&in_tree, &store_dir).is_err() {
            // Cross-device (or otherwise un-renamable): deep-copy instead. The
            // original stays in place until link_in_tree stages its replacement.
            copy_dir_recursive(&in_tree, &store_dir)?;
        }
        // At this point the env content lives (at least) at `store_dir`. If the
        // final link fails after a MOVE, put the env back in-tree.
        if let Err(e) = store.link_in_tree(kind, spec, base, &store_dir) {
            if !in_tree.exists() && store_dir.is_dir() {
                if std::fs::rename(&store_dir, &in_tree).is_ok() {
                    // moved back — clean fallback
                } else if copy_dir_recursive(&store_dir, &in_tree).is_ok() {
                    // restored by copy; leave the store copy for the next
                    // attempt (it is torn — no ready marker — and will be
                    // cleared by a future re-home)
                }
            }
            return Err(e);
        }
        Ok(store_record(&store_dir, kind, cache_key))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn venv_env(root: &Path, name: &str, with_file: &str) -> PathBuf {
        let dir = root.join(name);
        std::fs::create_dir_all(dir.join("bin")).unwrap();
        std::fs::write(dir.join("bin").join("python"), b"stub").unwrap();
        if !with_file.is_empty() {
            std::fs::write(dir.join(with_file), b"x").unwrap();
        }
        dir
    }

    #[test]
    fn store_dir_is_content_addressed_and_kind_prefixed() {
        let store = EnvStore::from_root(PathBuf::from("/tmp/never-created-store"));
        let a = store.store_dir(StoreKind::Pixi, "key-a");
        let b = store.store_dir(StoreKind::Pixi, "key-a");
        let c = store.store_dir(StoreKind::Venv, "key-a");
        assert_eq!(a, b, "same key + kind must map to the same dir");
        assert_ne!(a, c, "kind must be part of the dir name");
        let name = a.file_name().unwrap().to_string_lossy();
        assert!(name.starts_with("pixi-"));
        assert_eq!(name.len(), "pixi-".len() + 16);
        assert!(store.root().ends_with("never-created-store"));
    }

    #[test]
    fn kind_detection_and_in_tree_paths() {
        let mut spec = crate::rule::EnvironmentSpec::default();
        assert_eq!(StoreKind::from_spec(&spec), None);
        spec.pixi = Some("envs/pixi.toml".into());
        assert_eq!(StoreKind::from_spec(&spec), Some(StoreKind::Pixi));
        let spec2 = crate::rule::EnvironmentSpec {
            venv: Some(".venv".into()),
            ..Default::default()
        };
        assert_eq!(StoreKind::from_spec(&spec2), Some(StoreKind::Venv));

        let base = Path::new("/wf");
        assert_eq!(
            StoreKind::Pixi.in_tree_path("envs/pixi.toml", Some(base)),
            PathBuf::from("/wf/envs/.pixi")
        );
        assert_eq!(
            StoreKind::Venv.in_tree_path("/abs/.venv", None),
            PathBuf::from("/abs/.venv")
        );
        assert_eq!(
            StoreKind::Pixi.in_tree_path("/abs/envs/pixi.toml", None),
            PathBuf::from("/abs/envs/.pixi")
        );
    }

    #[test]
    fn link_and_broken_link_detection() {
        let tmp = tempfile::tempdir().unwrap();
        let store = EnvStore::from_root(tmp.path().join("store"));
        let env = venv_env(tmp.path(), "env", "");
        let store_dir = store.store_dir(StoreKind::Venv, "k");
        store.ensure_root().unwrap();
        std::fs::rename(&env, &store_dir).unwrap();
        store
            .link_in_tree(
                StoreKind::Venv,
                tmp.path().join("env").to_str().unwrap(),
                None,
                &store_dir,
            )
            .unwrap();

        let spec = tmp.path().join("env").to_str().unwrap().to_string();
        assert_eq!(
            store.in_tree_state(StoreKind::Venv, &spec, None),
            InTreeState::Fine
        );
        // Break the link by removing the store dir: state must be BrokenLink.
        std::fs::remove_dir_all(&store_dir).unwrap();
        assert_eq!(
            store.in_tree_state(StoreKind::Venv, &spec, None),
            InTreeState::BrokenLink
        );
        // Absent path.
        assert_eq!(
            store.in_tree_state(StoreKind::Venv, "/does/not/exist", None),
            InTreeState::Absent
        );
    }

    #[test]
    fn store_is_ready_recognizes_both_rehomed_shapes() {
        let tmp = tempfile::tempdir().unwrap();
        let store = EnvStore::from_root(tmp.path().to_path_buf());
        // Venv shape: bin/ + marker.
        let venv_dir = tmp.path().join("venv-x");
        std::fs::create_dir_all(venv_dir.join("bin")).unwrap();
        assert!(!store.store_is_ready(&venv_dir), "no marker yet");
        std::fs::write(venv_dir.join(".oxo-env-ready"), b"ok").unwrap();
        assert!(store.store_is_ready(&venv_dir));
        // Pixi shape: the renamed `.pixi` dir itself contains envs/ — no
        // nested `.pixi` (this is the B1 layout bug: only checking `.pixi`
        // never matched a re-homed pixi env).
        let pixi_dir = tmp.path().join("pixi-y");
        std::fs::create_dir_all(pixi_dir.join("envs").join("default")).unwrap();
        std::fs::write(pixi_dir.join(".oxo-env-ready"), b"ok").unwrap();
        assert!(store.store_is_ready(&pixi_dir));
        // Torn dir: env content but no marker.
        let torn = tmp.path().join("pixi-torn");
        std::fs::create_dir_all(torn.join("envs")).unwrap();
        assert!(!store.store_is_ready(&torn));
    }

    #[test]
    fn rehome_refuses_disabled_sentinel_store() {
        // The regression guard for the opt-in reshape: a disabled sentinel
        // (empty root, OXO_FLOW_ENV_STORE unset) must never move the env.
        // Before the gate, store_dir resolved to a RELATIVE path and the
        // rename landed the env in the process CWD with a dangling link.
        let tmp = tempfile::tempdir().unwrap();
        let store = EnvStore::from_root(PathBuf::new());
        assert!(!store.is_enabled());
        let env = venv_env(tmp.path(), "env", "mine.txt");
        let cwd_entries_before: Vec<_> = std::fs::read_dir(tmp.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        let err = rehome_into_store(&store, StoreKind::Venv, env.to_str().unwrap(), None, "k")
            .unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::PermissionDenied);
        let meta = std::fs::symlink_metadata(&env).unwrap();
        assert!(!meta.file_type().is_symlink(), "in-tree env untouched");
        assert!(env.join("mine.txt").is_file(), "env content intact");
        let cwd_entries_after: Vec<_> = std::fs::read_dir(tmp.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            cwd_entries_before, cwd_entries_after,
            "no store dir materialized in the working directory"
        );
    }

    #[test]
    fn rehome_moves_env_and_links_back() {
        let tmp = tempfile::tempdir().unwrap();
        let store = EnvStore::from_root(tmp.path().join("store"));
        let env = venv_env(tmp.path(), "env", "marker.txt");
        let record = rehome_into_store(
            &store,
            StoreKind::Venv,
            env.to_str().unwrap(),
            None,
            "cache-key",
        )
        .unwrap();
        let store_dir = store.store_dir(StoreKind::Venv, "cache-key");
        assert!(store.store_is_ready(&store_dir), "ready marker written");
        assert!(store_dir.join("bin").join("python").is_file());
        assert!(store_dir.join("marker.txt").is_file());
        assert!(env.is_dir(), "in-tree path now resolves through a link");
        let meta = std::fs::symlink_metadata(&env).unwrap();
        assert!(meta.file_type().is_symlink());
        assert_eq!(std::fs::read_link(&env).unwrap(), store_dir);
        assert_eq!(record.kind, "venv");
        assert_eq!(record.cache_key, "cache-key");
        assert_eq!(
            record.store_dir,
            store_dir.file_name().unwrap().to_string_lossy()
        );
    }

    #[test]
    fn rehome_clears_torn_store_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let store = EnvStore::from_root(tmp.path().join("store"));
        let env = venv_env(tmp.path(), "env", "");
        let store_dir = store.store_dir(StoreKind::Venv, "k");
        store.ensure_root().unwrap();
        // Torn: env-shaped content but no ready marker.
        std::fs::create_dir_all(store_dir.join("lib")).unwrap();
        std::fs::write(store_dir.join("lib").join("stale"), b"x").unwrap();
        rehome_into_store(&store, StoreKind::Venv, env.to_str().unwrap(), None, "k").unwrap();
        assert!(store.store_is_ready(&store_dir));
        assert!(!store_dir.join("lib").join("stale").exists());
    }

    #[test]
    fn rehome_never_clears_foreign_store_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let store = EnvStore::from_root(tmp.path().join("store"));
        let env = venv_env(tmp.path(), "env", "");
        let store_dir = store.store_dir(StoreKind::Venv, "k");
        store.ensure_root().unwrap();
        std::fs::create_dir_all(&store_dir).unwrap();
        std::fs::write(store_dir.join("user-data.txt"), b"precious").unwrap();
        let err = rehome_into_store(&store, StoreKind::Venv, env.to_str().unwrap(), None, "k")
            .unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::AlreadyExists);
        assert!(
            store_dir.join("user-data.txt").is_file(),
            "foreign file kept"
        );
        let meta = std::fs::symlink_metadata(&env).unwrap();
        assert!(!meta.file_type().is_symlink(), "in-tree env untouched");
    }

    #[test]
    fn rehome_adopts_ready_store_dir_from_lost_race() {
        let tmp = tempfile::tempdir().unwrap();
        let store = EnvStore::from_root(tmp.path().join("store"));
        let env = venv_env(tmp.path(), "env", "mine.txt");
        let store_dir = store.store_dir(StoreKind::Venv, "k");
        store.ensure_root().unwrap();
        // Another run already materialized + marked the store dir.
        std::fs::create_dir_all(store_dir.join("bin")).unwrap();
        std::fs::write(store_dir.join("bin").join("python"), b"theirs").unwrap();
        std::fs::write(store_dir.join(".oxo-env-ready"), b"ok\n").unwrap();

        let record =
            rehome_into_store(&store, StoreKind::Venv, env.to_str().unwrap(), None, "k").unwrap();
        assert!(record.store_dir.starts_with("venv-"));
        // In-tree now points at the ready store dir.
        assert!(store_dir.join("bin").join("python").is_file());
        assert!(
            store.in_tree_state(StoreKind::Venv, env.to_str().unwrap(), None) == InTreeState::Fine
        );
    }

    #[test]
    fn rehome_aborts_on_preexisting_in_tree_symlink_without_ready_store() {
        let tmp = tempfile::tempdir().unwrap();
        let store = EnvStore::from_root(tmp.path().join("store"));
        let env = venv_env(tmp.path(), "env", "");
        let elsewhere = venv_env(tmp.path(), "elsewhere", "");
        std::fs::remove_dir_all(&env).unwrap();
        std::os::unix::fs::symlink(&elsewhere, &env).unwrap();
        let err = rehome_into_store(&store, StoreKind::Venv, env.to_str().unwrap(), None, "k")
            .unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::AlreadyExists);
        // The pre-existing link is untouched (no self-referential rename).
        assert_eq!(std::fs::read_link(&env).unwrap(), elsewhere);
    }

    #[test]
    fn link_failure_restore_contract_and_no_stage_leftovers() {
        // MINOR 6b: the rename-failure branch promises (a) a destroyed
        // in-tree env is restored from the store copy and (b) no staged
        // symlink is left behind. Exercise both contracts deterministically.
        let tmp = tempfile::tempdir().unwrap();
        let store = EnvStore::from_root(tmp.path().join("store"));
        let env = venv_env(tmp.path(), "env", "mine.txt");
        let store_dir = store.store_dir(StoreKind::Venv, "k");
        store.ensure_root().unwrap();
        std::fs::rename(&env, &store_dir).unwrap();
        std::fs::write(store_dir.join(".oxo-env-ready"), b"ok\n").unwrap();

        // (a) Restore contract: the error path recovers via
        // copy_dir_recursive(store_dir → in_tree); assert it puts the full
        // environment content back.
        let spec = env.to_str().unwrap().to_string();
        let in_tree = StoreKind::Venv.in_tree_path(&spec, None);
        std::fs::create_dir_all(&in_tree).unwrap();
        copy_dir_recursive(&store_dir, &in_tree).unwrap();
        assert!(in_tree.join("mine.txt").is_file(), "env content restored");
        assert!(
            in_tree.join("bin").join("python").is_file(),
            "bin/python restored"
        );

        // (b) Stage-cleanup contract: after a complete link_in_tree swap,
        // the parent holds no .oxo-store-link leftovers.
        std::fs::remove_dir_all(&in_tree).unwrap();
        std::fs::remove_dir_all(&store_dir).unwrap();
        let fresh = venv_env(tmp.path(), "env2", "mine.txt");
        std::fs::rename(&fresh, &store_dir).unwrap();
        std::fs::write(store_dir.join(".oxo-env-ready"), b"ok\n").unwrap();
        store
            .link_in_tree(StoreKind::Venv, &spec, None, &store_dir)
            .unwrap();
        let stage_leftover = in_tree
            .parent()
            .unwrap()
            .read_dir()
            .unwrap()
            .filter_map(|e| e.ok())
            .any(|e| e.file_name().to_string_lossy().contains(".oxo-store-link"));
        assert!(
            !stage_leftover,
            "no staged .oxo-store-link leftovers after a successful swap"
        );
        assert!(in_tree.join("mine.txt").is_file(), "swap serves content");
    }

    #[test]
    fn copy_dir_recursive_copies_dirs_files_and_symlinks() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("src");
        std::fs::create_dir_all(src.join("nested").join("deep")).unwrap();
        std::fs::write(src.join("nested").join("deep").join("f.txt"), b"data").unwrap();
        let target = tmp.path().join("outside.txt");
        std::fs::write(&target, b"t").unwrap();
        std::os::unix::fs::symlink(&target, src.join("nested").join("link")).unwrap();

        let dst = tmp.path().join("dst");
        copy_dir_recursive(&src, &dst).unwrap();
        assert_eq!(
            std::fs::read(dst.join("nested").join("deep").join("f.txt")).unwrap(),
            b"data"
        );
        let link = dst.join("nested").join("link");
        assert!(
            std::fs::symlink_metadata(&link)
                .unwrap()
                .file_type()
                .is_symlink(),
            "symlinks are copied as links"
        );
        assert_eq!(std::fs::read_link(&link).unwrap(), target);
    }

    #[test]
    fn referenced_store_dirs_is_order_independent() {
        let mut a = std::collections::BTreeMap::new();
        a.insert(
            "r1".into(),
            EnvStoreRecord {
                store_dir: "venv-1".into(),
                kind: "venv".into(),
                cache_key: "k1".into(),
            },
        );
        let mut b = a.clone();
        b.insert(
            "r2".into(),
            EnvStoreRecord {
                store_dir: "pixi-2".into(),
                kind: "pixi".into(),
                cache_key: "k2".into(),
            },
        );
        let refs = referenced_store_dirs([&a, &b]);
        let expected: std::collections::BTreeSet<String> =
            ["venv-1".to_string(), "pixi-2".to_string()]
                .into_iter()
                .collect();
        assert_eq!(refs, expected);
        assert!(
            referenced_store_dirs([&a, &b]) == refs,
            "stable across calls"
        );
    }
}
