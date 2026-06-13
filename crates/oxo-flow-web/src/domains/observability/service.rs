//! Pure observability domain logic — zero HTTP dependency.
//!
//! Each function takes plain Rust types and returns `Result<T, String>`.
//! Suitable for reuse from handlers, CLI commands, or tests without
//! coupling to axum or any web framework.

use crate::domains::observability::types::*;

/// Build health check response with component status.
pub fn health_check(mode: &str, db_healthy: bool) -> HealthResponse {
    let uptime = std::time::Instant::now().elapsed().as_secs(); // approximation

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
            ai_provider: None,
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
            contact: "wangsx@traitome.com".into(),
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
        uptime_secs: 0,
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
    fn test_system_info() {
        let info = system_info();
        assert!(!info.version.is_empty());
        assert!(!info.os.is_empty());
    }
}
