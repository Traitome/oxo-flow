use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthResponse {
    pub status: String,
    pub version: String,
    pub mode: String,
    pub uptime_secs: u64,
    pub components: ComponentHealth,
    pub resources: ResourceInfo,
    pub license: LicenseInfo,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComponentHealth {
    pub database: ComponentStatus,
    pub filesystem: ComponentStatus,
    pub scheduler: Option<ComponentStatus>,
    pub ai_provider: Option<ComponentStatus>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComponentStatus {
    pub status: String,
    pub latency_ms: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourceInfo {
    pub cpu_pct: f64,
    pub memory_used_pct: f64,
    pub disk_used_pct: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LicenseInfo {
    pub license_type: String,
    pub valid: bool,
    pub commercial_use: String,
    pub contact: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SystemInfoResponse {
    pub version: String,
    pub rust_version: String,
    pub os: String,
    pub arch: String,
    pub pid: u32,
    pub uptime_secs: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuntimeMetricsResponse {
    pub uptime_secs: u64,
    pub version: String,
    pub pid: u32,
    pub os: String,
    pub arch: String,
    pub cpu_count: usize,
    pub total_requests: u64,
    pub active_workflows: i64,
    pub host: HostResources,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HostResources {
    pub cpu_usage_percent: f64,
    pub total_memory_mb: u64,
    pub used_memory_mb: u64,
    pub total_swap_mb: u64,
    pub used_swap_mb: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditLogQuery {
    pub days: Option<u8>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditLogResponse {
    pub entries: Vec<AuditEntry>,
    pub days: u8,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditEntry {
    pub timestamp: String,
    pub user: String,
    pub action: String,
    pub resource: String,
    pub result: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DataAnalysisRequest {
    pub paths: Vec<String>,
    pub max_depth: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DataAnalysisResponse {
    pub files: Vec<FileInfo>,
    pub summary: DataSummary,
    pub suggested_workflow: Option<WorkflowSuggestion>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileInfo {
    pub path: String,
    pub size: u64,
    pub format: String,
    pub format_confidence: f64,
    pub paired_with: Option<String>,
    pub sample_name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DataSummary {
    pub total_size: u64,
    pub formats_detected: Vec<String>,
    pub paired_end_detected: bool,
    pub strand_specific: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowSuggestion {
    pub template: String,
    pub confidence: f64,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReferenceRequest {
    pub genome: String,
    pub components: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReferenceResponse {
    pub found: Vec<String>,
    pub missing: Vec<String>,
    pub download_commands: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_health_response_roundtrip() {
        let resp = HealthResponse {
            status: "healthy".into(),
            version: "0.8.0".into(),
            mode: "server".into(),
            uptime_secs: 3600,
            components: ComponentHealth {
                database: ComponentStatus {
                    status: "ok".into(),
                    latency_ms: Some(5.0),
                },
                filesystem: ComponentStatus {
                    status: "ok".into(),
                    latency_ms: Some(2.0),
                },
                scheduler: Some(ComponentStatus {
                    status: "ok".into(),
                    latency_ms: Some(1.0),
                }),
                ai_provider: None,
            },
            resources: ResourceInfo {
                cpu_pct: 25.0,
                memory_used_pct: 50.0,
                disk_used_pct: 30.0,
            },
            license: LicenseInfo {
                license_type: "MIT".into(),
                valid: true,
                commercial_use: "yes".into(),
                contact: "admin@example.com".into(),
                message: "ok".into(),
            },
        };
        let json = serde_json::to_string(&resp).unwrap();
        let back: HealthResponse = serde_json::from_str(&json).unwrap();
        assert_eq!(back.status, resp.status);
    }

    #[test]
    fn test_system_info_response_roundtrip() {
        let resp = SystemInfoResponse {
            version: "0.8.0".into(),
            rust_version: "1.75".into(),
            os: "linux".into(),
            arch: "x86_64".into(),
            pid: 1234,
            uptime_secs: 3600,
        };
        let json = serde_json::to_string(&resp).unwrap();
        let back: SystemInfoResponse = serde_json::from_str(&json).unwrap();
        assert_eq!(back.version, resp.version);
    }

    #[test]
    fn test_runtime_metrics_response_roundtrip() {
        let resp = RuntimeMetricsResponse {
            uptime_secs: 3600,
            version: "0.8.0".into(),
            pid: 1234,
            os: "linux".into(),
            arch: "x86_64".into(),
            cpu_count: 8,
            total_requests: 100,
            active_workflows: 3,
            host: HostResources {
                cpu_usage_percent: 45.0,
                total_memory_mb: 16000,
                used_memory_mb: 8000,
                total_swap_mb: 4000,
                used_swap_mb: 500,
            },
        };
        let json = serde_json::to_string(&resp).unwrap();
        let back: RuntimeMetricsResponse = serde_json::from_str(&json).unwrap();
        assert_eq!(back.cpu_count, 8);
    }

    #[test]
    fn test_audit_log_response_roundtrip() {
        let resp = AuditLogResponse {
            entries: vec![AuditEntry {
                timestamp: "2024-01-01T00:00:00Z".into(),
                user: "admin".into(),
                action: "create".into(),
                resource: "pipeline/p1".into(),
                result: "success".into(),
            }],
            days: 7,
        };
        let json = serde_json::to_string(&resp).unwrap();
        let back: AuditLogResponse = serde_json::from_str(&json).unwrap();
        assert_eq!(back.days, 7);
    }

    #[test]
    fn test_data_analysis_response_roundtrip() {
        let resp = DataAnalysisResponse {
            files: vec![FileInfo {
                path: "/data/sample1.fastq".into(),
                size: 1024,
                format: "fastq".into(),
                format_confidence: 0.99,
                paired_with: Some("/data/sample2.fastq".into()),
                sample_name: Some("sample1".into()),
            }],
            summary: DataSummary {
                total_size: 2048,
                formats_detected: vec!["fastq".into()],
                paired_end_detected: true,
                strand_specific: Some(false),
            },
            suggested_workflow: Some(WorkflowSuggestion {
                template: "rna-seq".into(),
                confidence: 0.95,
                reason: "fastq files detected".into(),
            }),
        };
        let json = serde_json::to_string(&resp).unwrap();
        let back: DataAnalysisResponse = serde_json::from_str(&json).unwrap();
        assert_eq!(back.files.len(), 1);
    }

    #[test]
    fn test_reference_response_roundtrip() {
        let resp = ReferenceResponse {
            found: vec!["hg38".into()],
            missing: vec!["genome.fa".into()],
            download_commands: vec!["wget ...".into()],
        };
        let json = serde_json::to_string(&resp).unwrap();
        let back: ReferenceResponse = serde_json::from_str(&json).unwrap();
        assert_eq!(back.found.len(), 1);
    }
}
