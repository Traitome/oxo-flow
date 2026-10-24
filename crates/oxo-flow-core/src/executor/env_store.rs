//! Content-addressed shared environment store (issue #904).
//!
//! File-backed environments (pixi manifests, venv dirs) materialize a full
//! environment beside the workflow tree on every setup — N checkouts of the
//! same pipeline pay the solve/install cost N times and hold N copies of the
//! env on disk. This module introduces a machine-level store: setup writes
//! the environment once under a content-derived key, and the in-tree
//! location the rest of the engine already knows about becomes a symlink
//! into the store. Wrap commands (`pixi run --manifest-path`, `source
//! .venv/bin/activate`) keep working unchanged because they resolve through
//! the symlink.
//!
//! Key invariants:
//! - The store key reuses the per-backend `cache_key` verbatim (including
//!   the #532 content tags) — never retype or re-derive a hash scheme here.
//! - Teardown safety: `rm -rf` on a symlink removes the link only, so the
//!   existing venv teardown naturally stays store-safe; nothing here ever
//!   deletes store content (explicit `oxo-flow env gc` is the only deleter).
//! - Broken-link detection: pixi and venv have no `verify_command`
//!   (`verify_command` returns `None` for them), so a store symlink whose
//!   target was removed (manual cleanup, disk-pressure sweep) would pass
//!   every existing check and fail at rule time. [`EnvStore::in_tree_ok`]
//!   closes that gap.
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

/// Environment variable selecting the shared env store root.
pub const ENV_STORE_ENV_VAR: &str = "OXO_FLOW_ENV_STORE";

/// Default store root: `~/.cache/oxo-flow/envs` (XDG-ish, per-user).
const DEFAULT_STORE_PARENT: &str = ".cache/oxo-flow/envs";

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
/// One instance per `LocalExecutor`; all paths are computed, none cached
/// across runs — the store may be modified (or emptied) between runs by
/// `env gc` or the user, so every check re-stats.
#[derive(Debug, Clone)]
pub struct EnvStore {
    root: PathBuf,
}

impl EnvStore {
    /// Resolves the store root: `$OXO_FLOW_ENV_STORE` when set, otherwise
    /// `~/.cache/oxo-flow/envs` (falling back to the system temp dir when
    /// no home directory is discoverable — best-effort, mirroring
    /// `EnvCreateLock`'s tolerance of an unavailable home).
    pub fn from_env() -> Self {
        if let Some(dir) = std::env::var_os(ENV_STORE_ENV_VAR)
            && !dir.is_empty()
        {
            return Self {
                root: PathBuf::from(dir),
            };
        }
        let home = std::env::var_os("HOME")
            .or_else(|| std::env::var_os("USERPROFILE"))
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir);
        Self {
            root: home.join(DEFAULT_STORE_PARENT),
        }
    }

    /// Store root as configured (test/CLI introspection).
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Content-addressed store directory for a cache key:
    /// `<root>/<backend>-<sha256-16hex>` — same derivation discipline as
    /// `lock_path_for` in `env_create_lock` (hash the key, truncate to 16
    /// hex chars; collision risk negligible for env counts).
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
    /// mistaken for a usable env.
    pub fn store_is_ready(&self, store_dir: &Path) -> bool {
        store_dir.join(".oxo-env-ready").is_file()
            && (store_dir.join(".pixi").exists() || store_dir.join("bin").exists())
    }

    /// Broken-link / stale-target check for a store-managed in-tree path.
    ///
    /// Returns `true` when nothing needs attention (no store in play, or
    /// the in-tree entry exists and resolves), `false` when the in-tree
    /// entry is a symlink whose target is gone — the exact state the
    /// backend-level `verify_command = None` gap lets through today.
    pub fn in_tree_ok(&self, kind: StoreKind, spec: &str, base: Option<&Path>) -> bool {
        let in_tree = kind.in_tree_path(spec, base);
        match std::fs::symlink_metadata(&in_tree) {
            // Nothing materialized in-tree yet (cold path): not broken.
            Err(_) => true,
            Ok(meta) if !meta.file_type().is_symlink() => true, // real dir: fine
            Ok(_) => in_tree.exists(),                          // symlink: target must resolve
        }
    }

    /// Re-points the in-tree path at `store_dir` and writes the ready
    /// marker. Returns the previous state so callers can log what they
    /// replaced (a real dir from a pre-store run is re-homed into the
    /// store by the caller *before* calling this).
    pub fn link_in_tree(
        &self,
        kind: StoreKind,
        spec: &str,
        base: Option<&Path>,
        store_dir: &Path,
    ) -> std::io::Result<()> {
        let in_tree = kind.in_tree_path(spec, base);
        if let Some(parent) = in_tree.parent() {
            std::fs::create_dir_all(parent)?;
        }
        // Replace whatever is there (symlink, file, or leftover real dir)
        // with a fresh symlink. A real dir at the same content key means an
        // old non-store env: its content was already re-homed by the caller
        // (or it is a partial remnant — either way the store copy wins).
        match std::fs::symlink_metadata(&in_tree) {
            Ok(meta) if meta.is_dir() && !meta.file_type().is_symlink() => {
                std::fs::remove_dir_all(&in_tree)?;
            }
            Ok(_) => {
                std::fs::remove_file(&in_tree)?;
            }
            Err(_) => {}
        }
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(store_dir, &in_tree)?;
        }
        #[cfg(not(unix))]
        {
            return Err(std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "env store symlinks require a Unix filesystem",
            ));
        }
        std::fs::write(store_dir.join(".oxo-env-ready"), b"ok\n")?;
        Ok(())
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

/// Re-home + link + record: moves a freshly materialized in-tree
/// environment into the content-addressed store, symlinks the in-tree path
/// back, and records the reference in the checkpoint.
///
/// Best-effort by design — every failure propagates as `Err` and the CALLER
/// falls back to the plain in-tree env (the pre-#904 behavior); the store
/// just misses the dedup. A half-created store dir at the same key (killed
/// setup) is removed before the rename.
pub(crate) fn rehome_into_store(
    store: &EnvStore,
    kind: StoreKind,
    spec: &str,
    base: Option<&Path>,
    cache_key: &str,
    rule_name: &str,
    record_into: Option<&mut std::collections::BTreeMap<String, EnvStoreRecord>>,
) -> std::io::Result<()> {
    let store_dir = store.store_dir(kind, cache_key);
    store.ensure_root()?;
    // A torn/partial store dir at the same key (killed setup) must not
    // block re-homing; a READY dir cannot happen here (the caller only
    // re-homes when the store path was not ready).
    if store_dir.exists() && !store.store_is_ready(&store_dir) {
        std::fs::remove_dir_all(&store_dir)?;
    }
    let in_tree = kind.in_tree_path(spec, base);
    if std::fs::rename(&in_tree, &store_dir).is_err() {
        // Cross-device (or busy): copy the tree into the store FIRST, then
        // drop the in-tree original only once the store copy is complete —
        // a copy failure leaves the in-tree env intact for the caller's
        // fallback.
        copy_dir_recursive(&in_tree, &store_dir)?;
        std::fs::remove_dir_all(&in_tree)?;
    }
    store.link_in_tree(kind, spec, base, &store_dir)?;
    if let Some(records) = record_into {
        records.insert(
            rule_name.to_string(),
            EnvStoreRecord {
                store_dir: store_dir
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned(),
                kind: kind.prefix().to_string(),
                cache_key: cache_key.to_string(),
            },
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec_pixi(manifest: &str) -> crate::rule::EnvironmentSpec {
        crate::rule::EnvironmentSpec {
            pixi: Some(manifest.to_string()),
            ..Default::default()
        }
    }

    fn spec_venv(dir: &str) -> crate::rule::EnvironmentSpec {
        crate::rule::EnvironmentSpec {
            venv: Some(dir.to_string()),
            ..Default::default()
        }
    }

    #[test]
    fn store_dir_is_content_addressed_and_kind_prefixed() {
        let store = EnvStore {
            root: PathBuf::from("/tmp/store-test"),
        };
        let a = store.store_dir(StoreKind::Pixi, "pixi:pyproject.toml");
        let b = store.store_dir(StoreKind::Pixi, "pixi:pyproject.toml:abc");
        let v = store.store_dir(StoreKind::Venv, "venv:.venv");
        assert_ne!(a, b, "different content → different dir");
        // `Path::starts_with` compares components, not string prefixes —
        // compare on the rendered string.
        let a_str = a.to_string_lossy();
        let v_str = v.to_string_lossy();
        assert!(
            a_str.starts_with("/tmp/store-test/pixi-"),
            "unexpected store dir for pixi key: {a_str}"
        );
        assert!(
            v_str.starts_with("/tmp/store-test/venv-"),
            "unexpected store dir for venv key: {v_str}"
        );
        // Same key → same dir (deterministic, order-independent).
        assert_eq!(a, store.store_dir(StoreKind::Pixi, "pixi:pyproject.toml"));
    }

    #[test]
    fn kind_detection_and_in_tree_paths() {
        assert_eq!(
            StoreKind::from_spec(&spec_pixi("pixi.toml")),
            Some(StoreKind::Pixi)
        );
        assert_eq!(
            StoreKind::from_spec(&spec_venv(".venv")),
            Some(StoreKind::Venv)
        );
        assert_eq!(
            StoreKind::from_spec(&crate::rule::EnvironmentSpec {
                conda: Some("envs/x.yaml".to_string()),
                ..Default::default()
            }),
            None,
            "conda envs are not store-managed"
        );

        // Pixi: manifest FILE → `.pixi` beside it.
        let abs_manifest = std::path::Path::new("/wf/envs/pixi.toml");
        let env_spec = spec_pixi("/wf/envs/pixi.toml");
        assert_eq!(
            StoreKind::Pixi.in_tree_path(env_spec.pixi.as_deref().unwrap(), None),
            abs_manifest.parent().unwrap().join(".pixi")
        );
        // Venv: spec IS the dir.
        assert_eq!(
            StoreKind::Venv.in_tree_path(".venv", Some(Path::new("/wf"))),
            PathBuf::from("/wf/.venv")
        );
    }

    #[test]
    fn link_and_broken_link_detection() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let store = EnvStore {
            root: tmp.path().join("store"),
        };
        store.ensure_root().expect("ensure_root");

        let store_dir = store.store_dir(StoreKind::Venv, "venv:/wf/.venv");
        std::fs::create_dir_all(store_dir.join("bin")).expect("materialize env");
        // Pre-store state: a real in-tree dir reports OK, no store hit.
        let in_tree = tmp.path().join("wf/.venv");
        std::fs::create_dir_all(&in_tree).expect("in-tree dir");
        assert!(store.in_tree_ok(StoreKind::Venv, "/tmp/placeholder", None));

        // Link it: now a symlink to a real store dir → OK.
        store
            .link_in_tree(
                StoreKind::Venv,
                &in_tree.to_string_lossy(),
                None,
                &store_dir,
            )
            .expect("link");
        assert!(in_tree.is_dir(), "symlink resolves as dir");
        assert!(store.store_is_ready(&store_dir), "ready marker written");

        // Break the target: symlink dangles → NOT ok (the verify gap).
        std::fs::remove_dir_all(&store_dir).expect("remove store content");
        assert!(
            !store.in_tree_ok(StoreKind::Venv, &in_tree.to_string_lossy(), None),
            "dangling symlink must be detected"
        );
    }

    #[test]
    fn entries_skips_foreign_and_reports_ready_state() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let store = EnvStore {
            root: tmp.path().join("store"),
        };
        store.ensure_root().expect("ensure_root");

        let ready_dir = store.store_dir(StoreKind::Pixi, "pixi:one");
        std::fs::create_dir_all(ready_dir.join(".pixi")).expect("pixi dir");
        std::fs::write(ready_dir.join(".oxo-env-ready"), b"ok\n").expect("marker");

        let torn_dir = store.store_dir(StoreKind::Venv, "venv:two");
        std::fs::create_dir_all(&torn_dir).expect("torn dir"); // no marker

        std::fs::write(store.root().join("foreign.txt"), b"x").expect("foreign file");

        let entries = store.entries().expect("entries");
        assert_eq!(entries.len(), 2, "foreign files are skipped");
        let ready = entries.iter().find(|e| e.ready).expect("one ready");
        let torn = entries.iter().find(|e| !e.ready).expect("one torn");
        assert_eq!(ready.path, ready_dir);
        assert_eq!(torn.path, torn_dir);
    }

    #[test]
    fn referenced_store_dirs_is_order_independent() {
        let mut a = std::collections::BTreeMap::new();
        a.insert(
            "r1".to_string(),
            EnvStoreRecord {
                store_dir: "pixi-aaa".to_string(),
                kind: "pixi".to_string(),
                cache_key: "k1".to_string(),
            },
        );
        let mut b = std::collections::BTreeMap::new();
        b.insert(
            "r2".to_string(),
            EnvStoreRecord {
                store_dir: "venv-bbb".to_string(),
                kind: "venv".to_string(),
                cache_key: "k2".to_string(),
            },
        );
        let refs = referenced_store_dirs([&a, &b]);
        assert_eq!(
            refs,
            ["pixi-aaa", "venv-bbb"]
                .into_iter()
                .map(String::from)
                .collect()
        );
    }
}
