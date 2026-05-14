use serde::{Deserialize, Serialize};
use std::sync::Mutex;
use sysinfo::System;

lazy_static::lazy_static! {
    static ref SYS: Mutex<System> = Mutex::new(System::new_all());
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HostResources {
    pub cpu_usage_percent: f32,
    pub total_memory_mb: u64,
    pub used_memory_mb: u64,
    pub total_swap_mb: u64,
    pub used_swap_mb: u64,
}

/// Retrieve current host resource metrics.
pub fn get_host_resources() -> HostResources {
    let mut sys = SYS.lock().unwrap();
    sys.refresh_cpu_usage();
    sys.refresh_memory();

    let cpu_usage_percent = sys.global_cpu_usage();
    let total_memory_mb = sys.total_memory() / 1024 / 1024;
    let used_memory_mb = sys.used_memory() / 1024 / 1024;
    let total_swap_mb = sys.total_swap() / 1024 / 1024;
    let used_swap_mb = sys.used_swap() / 1024 / 1024;

    HostResources {
        cpu_usage_percent,
        total_memory_mb,
        used_memory_mb,
        total_swap_mb,
        used_swap_mb,
    }
}
