use crate::platform::{self, Gpu, MemoryDetail};
use serde::Serialize;
use std::{
    collections::{HashMap, VecDeque},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    thread,
    time::{Duration, Instant},
};
use sysinfo::{Components, ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind, Users};

pub const HISTORY: usize = 120;

#[derive(Clone, Debug, Default, Serialize)]
pub struct Temperature {
    pub label: String,
    pub celsius: Option<f32>,
}

fn valid_temperature(value: Option<f32>) -> Option<f32> {
    value.filter(|v| v.is_finite() && *v > 0.0 && *v <= 150.0)
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct Process {
    pub pid: u32,
    pub name: String,
    pub cpu: Option<f32>,
    pub memory: Option<u64>,
    pub memory_footprint: Option<u64>,
    pub status: String,
    pub user: String,
    pub executable: String,
    pub virtual_memory: Option<u64>,
    pub elapsed: Option<u64>,
}

impl Process {
    pub fn displayed_memory(&self) -> Option<u64> {
        if cfg!(target_os = "macos") {
            self.memory_footprint
        } else {
            self.memory
        }
    }
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct Snapshot {
    pub sequence: u64,
    pub host: String,
    pub os: String,
    pub cpu_name: String,
    pub cpu_usage: Option<f32>,
    pub cores: Vec<Option<f32>>,
    pub memory_total: Option<u64>,
    pub memory_used: Option<u64>,
    pub memory_available: Option<u64>,
    pub swap_total: Option<u64>,
    pub swap_used: Option<u64>,
    pub memory_detail: MemoryDetail,
    pub cpu_caches: String,
    pub gpus: Vec<Gpu>,
    pub temperatures: Vec<Temperature>,
    pub processes: Vec<Process>,
    pub load: Option<[f64; 3]>,
    pub uptime: Option<u64>,
    pub collection_ms: f64,
    pub cpu_history: VecDeque<u64>,
    pub memory_history: VecDeque<u64>,
    #[serde(skip)]
    pub collected_at: Option<Instant>,
}

pub struct Collector {
    system: System,
    #[cfg(target_os = "macos")]
    cpu_sampler: platform::CpuSampler,
    snapshot: Snapshot,
    users: Users,
    components: Components,
    #[cfg(target_os = "macos")]
    process_samples: HashMap<u32, platform::ProcessSample>,
    #[cfg(not(target_os = "macos"))]
    process_samples: HashMap<u32, u64>,
}

impl Default for Collector {
    fn default() -> Self {
        Self::new()
    }
}

impl Collector {
    pub fn new() -> Self {
        let mut system = System::new();
        system.refresh_cpu_all();
        system.refresh_processes_specifics(ProcessesToUpdate::All, true, process_refresh());
        let snapshot = Snapshot {
            host: System::host_name().unwrap_or_else(|| "localhost".into()),
            os: System::long_os_version().unwrap_or_else(|| std::env::consts::OS.into()),
            cpu_name: system
                .cpus()
                .first()
                .map(|c| c.brand().to_owned())
                .unwrap_or_default(),
            cpu_caches: platform::cpu_caches(),
            ..Default::default()
        };
        #[cfg(target_os = "macos")]
        let process_samples = system
            .processes()
            .keys()
            .filter_map(|pid| {
                platform::process_sample(pid.as_u32()).map(|sample| (pid.as_u32(), sample))
            })
            .collect();
        #[cfg(not(target_os = "macos"))]
        let process_samples = system
            .processes()
            .iter()
            .map(|(pid, p)| (pid.as_u32(), p.start_time()))
            .collect();
        #[cfg(target_os = "macos")]
        let mut cpu_sampler = platform::CpuSampler::default();
        #[cfg(target_os = "macos")]
        cpu_sampler.sample();
        Self {
            #[cfg(target_os = "macos")]
            cpu_sampler,
            process_samples,
            system,
            snapshot,
            users: Users::new_with_refreshed_list(),
            components: Components::new_with_refreshed_list(),
        }
    }

    pub fn sample(&mut self) -> Snapshot {
        let start = Instant::now();
        #[cfg(not(target_os = "macos"))]
        {
            self.system.refresh_cpu_usage();
            self.system.refresh_memory();
        }
        self.system
            .refresh_processes_specifics(ProcessesToUpdate::All, true, process_refresh());
        let s = &mut self.snapshot;
        s.sequence += 1;
        #[cfg(target_os = "macos")]
        {
            (s.cpu_usage, s.cores) = self.cpu_sampler.sample();
        }
        #[cfg(not(target_os = "macos"))]
        {
            s.cpu_usage = Some(self.system.global_cpu_usage())
                .filter(|v| v.is_finite() && !self.system.cpus().is_empty());
            s.cores = self
                .system
                .cpus()
                .iter()
                .map(|c| Some(c.cpu_usage()).filter(|v| v.is_finite()))
                .collect();
        }
        let (memory, detail) = platform::memory(&self.system);
        s.memory_total = memory.total;
        s.memory_used = memory.used;
        s.memory_available = memory.available;
        s.swap_total = memory.swap_total;
        s.swap_used = memory.swap_used;
        s.memory_detail = detail;
        s.gpus = platform::gpus();
        self.components.refresh(true);
        s.temperatures = self
            .components
            .iter()
            .map(|c| Temperature {
                label: c.label().chars().filter(|c| !c.is_control()).collect(),
                celsius: valid_temperature(c.temperature()),
            })
            .collect();
        // Driver readings supplement component sensors; labels identify the source.
        for gpu in &s.gpus {
            if let Some(celsius) = valid_temperature(gpu.temperature.map(|v| v as f32)) {
                s.temperatures.push(Temperature {
                    label: format!("GPU {} ({})", gpu.name, gpu.source),
                    celsius: Some(celsius),
                });
            }
        }
        s.temperatures.sort_by(|a, b| {
            b.celsius
                .partial_cmp(&a.celsius)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.label.cmp(&b.label))
        });
        let mut next_process_samples = HashMap::new();
        s.processes = self
            .system
            .processes()
            .iter()
            .map(|(pid, p)| {
                #[cfg(target_os = "macos")]
                let (cpu, memory, virtual_memory, memory_footprint) =
                    match platform::process_sample(pid.as_u32()) {
                        Some(current) if current.identity.0 == p.start_time() => {
                            let cpu = self
                                .process_samples
                                .get(&pid.as_u32())
                                .and_then(|old| current.cpu_since(*old));
                            next_process_samples.insert(pid.as_u32(), current);
                            (
                                cpu,
                                Some(current.resident),
                                Some(current.virtual_size),
                                current.footprint,
                            )
                        }
                        _ => (None, None, None, None),
                    };
                #[cfg(not(target_os = "macos"))]
                let (cpu, memory, virtual_memory, memory_footprint) = {
                    let available = !matches!(p.status(), sysinfo::ProcessStatus::Unknown(_));
                    let warmed = self.process_samples.get(&pid.as_u32()) == Some(&p.start_time());
                    if available {
                        next_process_samples.insert(pid.as_u32(), p.start_time());
                    }
                    (
                        available
                            .then_some(p.cpu_usage())
                            .filter(|v| warmed && v.is_finite() && *v >= 0.0),
                        available.then_some(p.memory()),
                        available.then_some(p.virtual_memory()),
                        None,
                    )
                };
                Process {
                    pid: pid.as_u32(),
                    name: p
                        .name()
                        .to_string_lossy()
                        .chars()
                        .filter(|c| !c.is_control())
                        .collect(),
                    cpu,
                    memory,
                    memory_footprint,
                    status: p.status().to_string(),
                    user: p
                        .user_id()
                        .map(|uid| {
                            self.users
                                .get_user_by_id(uid)
                                .map(|u| u.name().to_owned())
                                .unwrap_or_else(|| uid.to_string())
                        })
                        .unwrap_or_else(|| "?".into()),
                    executable: p
                        .exe()
                        .map(|path| {
                            path.to_string_lossy()
                                .chars()
                                .filter(|c| !c.is_control())
                                .collect()
                        })
                        .unwrap_or_default(),
                    virtual_memory,
                    elapsed: (p.start_time() > 0).then(|| p.run_time()),
                }
            })
            .collect();
        self.process_samples = next_process_samples;
        s.load = platform::load_average();
        // sysinfo uses zero on a failed boot-time query, which would otherwise
        // make uptime look like the number of seconds since the Unix epoch.
        s.uptime = (System::boot_time() > 0).then(System::uptime);
        if let Some(usage) = s.cpu_usage {
            push_history(&mut s.cpu_history, usage.round() as u64);
        } else {
            s.cpu_history.clear();
        }
        if let Some(fraction) = memory_ratio(s.memory_used, s.memory_total) {
            push_history(&mut s.memory_history, (fraction * 100.0).round() as u64);
        } else {
            s.memory_history.clear();
        }
        s.collection_ms = start.elapsed().as_secs_f64() * 1000.0;
        s.collected_at = Some(Instant::now());
        s.clone()
    }
}

fn process_refresh() -> ProcessRefreshKind {
    let refresh = ProcessRefreshKind::nothing()
        .with_user(UpdateKind::OnlyIfNotSet)
        .with_exe(UpdateKind::OnlyIfNotSet);
    #[cfg(target_os = "macos")]
    {
        refresh
    }
    #[cfg(not(target_os = "macos"))]
    {
        refresh.with_cpu().with_memory()
    }
}

fn push_history(history: &mut VecDeque<u64>, value: u64) {
    if history.len() == HISTORY {
        history.pop_front();
    }
    history.push_back(value);
}

// Only the newest snapshot is retained. Rendering never waits on the collector
// and pausing the display cannot build an unbounded queue.
pub struct Worker {
    pub latest: Arc<Mutex<Option<Snapshot>>>,
    stop: Arc<AtomicBool>,
    thread: Option<thread::JoinHandle<()>>,
}

impl Worker {
    pub fn start(interval: Duration) -> Self {
        let latest = Arc::new(Mutex::new(None));
        let stop = Arc::new(AtomicBool::new(false));
        let output = latest.clone();
        let stopping = stop.clone();
        let thread = thread::spawn(move || {
            let mut collector = Collector::new();
            thread::park_timeout(interval.max(sysinfo::MINIMUM_CPU_UPDATE_INTERVAL));
            while !stopping.load(Ordering::Relaxed) {
                let start = Instant::now();
                let snapshot = collector.sample();
                if let Ok(mut slot) = output.lock() {
                    *slot = Some(snapshot);
                }
                thread::park_timeout(interval.saturating_sub(start.elapsed()));
            }
        });
        Self {
            latest,
            stop,
            thread: Some(thread),
        }
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            thread.thread().unpark();
            let _ = thread.join();
        }
    }
}

pub fn memory_ratio(used: Option<u64>, total: Option<u64>) -> Option<f64> {
    used.zip(total)
        .filter(|(_, total)| *total > 0)
        .map(|(used, total)| ratio(used, total))
}

pub fn ratio(used: u64, total: u64) -> f64 {
    if total == 0 {
        0.0
    } else {
        (used as f64 / total as f64).clamp(0.0, 1.0)
    }
}

#[cfg(any(target_os = "macos", test))]
pub fn bytes(value: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut n = value as f64;
    let mut i = 0;
    while n >= 1024.0 && i < UNITS.len() - 1 {
        n /= 1024.0;
        i += 1;
    }
    if i == 0 {
        format!("{value} B")
    } else {
        format!("{n:.1} {}", UNITS[i])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bounded_history_preserves_recent_samples() {
        let mut history = VecDeque::new();
        for i in 0..200 {
            push_history(&mut history, i);
        }
        assert_eq!(history.len(), HISTORY);
        assert_eq!(history.front(), Some(&80));
        assert_eq!(history.back(), Some(&199));
    }
    #[test]
    fn invalid_sensor_values_stay_unavailable() {
        for value in [
            None,
            Some(f32::NAN),
            Some(f32::INFINITY),
            Some(0.0),
            Some(-1.0),
            Some(151.0),
        ] {
            assert_eq!(valid_temperature(value), None);
        }
        assert_eq!(valid_temperature(Some(72.5)), Some(72.5));
    }
    #[test]
    fn units_and_ratios_are_safe() {
        assert_eq!(bytes(1073741824), "1.0 GiB");
        assert_eq!(ratio(1, 0), 0.0);
        assert_eq!(ratio(20, 10), 1.0);
        assert_eq!(ratio(1, 4), 0.25);
        assert_eq!(memory_ratio(Some(0), Some(100)), Some(0.0));
        assert_eq!(memory_ratio(None, Some(100)), None);
        assert_eq!(memory_ratio(Some(10), Some(0)), None);
        assert_eq!(memory_ratio(Some(10), None), None);
    }
}
