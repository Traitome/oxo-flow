//! Pure observability domain logic — zero HTTP dependency.
//!
//! Each function takes plain Rust types and returns `Result<T, String>`.
//! Suitable for reuse from handlers, CLI commands, or tests without
//! coupling to axum or any web framework.

use std::sync::OnceLock;
use std::time::Instant;

use crate::domains::observability::types::*;

/// Process start time, initialized on the first observability call so the
/// uptime reported by `/api/health` and `/api/system` is meaningful even if
/// the server startup path does not explicitly seed it.
static START_TIME: OnceLock<Instant> = OnceLock::new();

pub fn uptime_secs() -> u64 {
    START_TIME.get_or_init(Instant::now).elapsed().as_secs()
}

/// Build health check response with component status.
pub fn health_check(mode: &str, db_healthy: bool) -> HealthResponse {
    health_check_with(
        mode,
        db_healthy,
        crate::infra::crypto::master_key_configured(),
    )
}

/// Pure core of [`health_check`] — `encrypted_keys` is injected so tests need
/// no environment mutation (mirrors `effective_bind_host_with` in lib.rs).
pub fn health_check_with(mode: &str, db_healthy: bool, encrypted_keys: bool) -> HealthResponse {
    build_health(mode, db_healthy, encrypted_keys, engine_version_info())
}

/// Shared builder — `engine` is injected so the pure core stays testable
/// without touching the process-global probe cache.
fn build_health(
    mode: &str,
    db_healthy: bool,
    encrypted_keys: bool,
    engine: Option<EngineBinaryInfo>,
) -> HealthResponse {
    let uptime = uptime_secs();

    HealthResponse {
        status: if db_healthy {
            "ok".into()
        } else {
            "degraded".into()
        },
        version: env!("CARGO_PKG_VERSION").into(),
        mode: mode.into(),
        uptime_secs: uptime,
        components: ComponentHealth {
            database: ComponentStatus {
                status: if db_healthy {
                    "ok".into()
                } else {
                    "error".into()
                },
                latency_ms: None,
            },
            filesystem: ComponentStatus {
                status: "ok".into(),
                latency_ms: None,
            },
            scheduler: None,
            ai_provider: {
                let config = crate::ai_provider::AiProviderRegistry::global().get_config();
                if config.is_configured {
                    Some(crate::domains::observability::types::ComponentStatus {
                        status: "connected".into(),
                        latency_ms: None,
                    })
                } else {
                    None
                }
            },
            ai_key_storage: if encrypted_keys {
                "encrypted".into()
            } else {
                "plaintext".into()
            },
            engine,
        },
        resources: ResourceInfo {
            cpu_pct: 0.0,
            memory_used_pct: 0.0,
            disk_used_pct: 0.0,
        },
        license: LicenseInfo {
            license_type: "academic".into(),
            valid: true,
            commercial_use: "requires_authorization".into(),
            contact: "w_shixiang@163.com".into(),
            message: "Free for academic use. Commercial use requires authorization.".into(),
        },
    }
}

/// The engine-binary info carried in `/api/health` and `/api/system`
/// (issue #579), from the cached startup probe.
pub fn engine_version_info() -> Option<EngineBinaryInfo> {
    let probe = crate::executor::engine_version_probe()?;
    Some(EngineBinaryInfo {
        version: probe.version.clone(),
        path: probe.binary.clone(),
        version_mismatch: probe.major_minor_mismatch(env!("CARGO_PKG_VERSION")),
    })
}

/// Build system info response.
pub fn system_info() -> SystemInfoResponse {
    SystemInfoResponse {
        version: env!("CARGO_PKG_VERSION").into(),
        rust_version: option_env!("CARGO_PKG_RUST_VERSION")
            .unwrap_or("unknown")
            .into(),
        os: std::env::consts::OS.into(),
        arch: std::env::consts::ARCH.into(),
        pid: std::process::id(),
        uptime_secs: uptime_secs(),
        engine: engine_version_info(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_health_check() {
        let h = health_check("personal", true);
        assert_eq!(h.status, "ok");
        assert_eq!(h.components.database.status, "ok");
        assert_eq!(h.license.license_type, "academic");
    }

    #[test]
    fn test_health_check_degraded() {
        let h = health_check("team", false);
        assert_eq!(h.status, "degraded");
        assert_eq!(h.components.database.status, "error");
    }

    #[test]
    fn test_health_reports_ai_key_storage_flag() {
        // The flag is the only API-visible signal that third-party AI keys
        // are being written to the database unencrypted (issue #205 audit).
        let plaintext = health_check_with("personal", true, false);
        assert_eq!(plaintext.components.ai_key_storage, "plaintext");

        let encrypted = health_check_with("personal", true, true);
        assert_eq!(encrypted.components.ai_key_storage, "encrypted");
    }

    #[test]
    fn test_system_info() {
        let info = system_info();
        assert!(!info.version.is_empty());
        assert!(!info.os.is_empty());
    }

    #[test]
    fn test_engine_version_info_from_probe_cache() {
        // A probe result injected into the builder surfaces as EngineBinaryInfo
        // with the mismatch flag computed against the server version.
        let engine = EngineBinaryInfo {
            version: "0.18.0".into(),
            path: "/usr/local/bin/oxo-flow".into(),
            version_mismatch: true,
        };
        let h = build_health("personal", true, true, Some(engine.clone()));
        assert_eq!(
            h.components.engine.as_ref().expect("engine set").version,
            "0.18.0"
        );

        // system_info always reflects the probe cache; in tests the probe has
        // not run, so it must stay None rather than fabricating a version.
        assert_eq!(system_info().engine, engine_version_info());
        assert_eq!(engine_version_info(), None, "no init in this test binary");
    }

    #[test]
    fn test_build_health_without_engine_probe() {
        let h = build_health("personal", true, true, None);
        assert!(h.components.engine.is_none());
        assert_eq!(h.status, "ok");
    }

    #[test]
    fn test_uptime_secs_is_small_for_fresh_process() {
        // The process-relative origin (shared with /api/health and
        // /api/system) must stay far below HOST boot uptime — /api/metrics
        // previously used sysinfo::System::uptime() and reported host boot
        // time (~27 days) for a server up for hours.
        assert!(uptime_secs() < 3600, "test process just started");
    }
}
