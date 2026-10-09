use serde::Serialize;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
pub use macos::{process_sample, CpuSampler, ProcessSample};

#[derive(Clone, Debug, Default, Serialize)]
pub struct MemoryDetail {
    pub cache_bytes: Option<u64>,
    pub compressed_bytes: Option<u64>,
    pub label: String,
    pub app_bytes: Option<u64>,
    pub other_bytes: Option<u64>,
    pub free_bytes: Option<u64>,
    pub wired_bytes: Option<u64>,
    pub cached_files_bytes: Option<u64>,
}

#[derive(Clone, Debug, Serialize)]
pub struct Gpu {
    pub name: String,
    pub utilization: Option<f64>,
    pub memory_used: Option<u64>,
    pub memory_total: Option<u64>,
    pub temperature: Option<f64>,
    pub source: String,
}

#[derive(Default)]
pub struct MemoryReading {
    pub total: Option<u64>,
    pub used: Option<u64>,
    pub available: Option<u64>,
    pub swap_total: Option<u64>,
    pub swap_used: Option<u64>,
}

pub fn memory(_system: &sysinfo::System) -> (MemoryReading, MemoryDetail) {
    #[cfg(target_os = "macos")]
    {
        macos::memory()
    }
    #[cfg(not(target_os = "macos"))]
    {
        let ready = _system.total_memory() > 0;
        let reading = MemoryReading {
            total: ready.then(|| _system.total_memory()),
            used: ready.then(|| _system.used_memory()),
            available: ready.then(|| _system.available_memory()),
            swap_total: ready.then(|| _system.total_swap()),
            swap_used: ready.then(|| _system.used_swap()),
        };
        #[cfg(target_os = "linux")]
        let detail = linux::memory();
        #[cfg(not(target_os = "linux"))]
        let detail = MemoryDetail::default();
        (reading, detail)
    }
}

pub fn gpus() -> Vec<Gpu> {
    #[cfg(target_os = "macos")]
    {
        macos::gpus()
    }
    #[cfg(target_os = "linux")]
    {
        linux::gpus()
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    Vec::new()
}

pub fn cpu_caches() -> String {
    #[cfg(target_os = "macos")]
    {
        macos::cpu_caches()
    }
    #[cfg(target_os = "linux")]
    {
        linux::cpu_caches()
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    "CPU cache topology unavailable".into()
}

pub fn load_average() -> Option<[f64; 3]> {
    #[cfg(target_os = "macos")]
    {
        macos::load_average()
    }
    #[cfg(not(target_os = "macos"))]
    {
        let load = sysinfo::System::load_average();
        let values = [load.one, load.five, load.fifteen];
        values
            .iter()
            .all(|v| v.is_finite() && *v >= 0.0)
            .then_some(values)
    }
}
