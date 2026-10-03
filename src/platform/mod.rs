use serde::Serialize;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;

#[derive(Clone, Debug, Default, Serialize)]
pub struct MemoryDetail {
    pub cache_bytes: Option<u64>,
    pub compressed_bytes: Option<u64>,
    pub label: String,
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

pub fn memory() -> MemoryDetail {
    #[cfg(target_os = "macos")]
    {
        macos::memory()
    }
    #[cfg(target_os = "linux")]
    {
        linux::memory()
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    MemoryDetail {
        label: "Cache unavailable".into(),
        ..Default::default()
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
