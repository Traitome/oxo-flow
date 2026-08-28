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

fn uptime_secs() -> u64 {
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
}
