use crate::platform::{self, Gpu, MemoryDetail};
use serde::Serialize;
use std::{
    collections::VecDeque,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    thread,
    time::{Duration, Instant},
};
use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind, Users};

pub const HISTORY: usize = 120;

#[derive(Clone, Debug, Default, Serialize)]
pub struct Process {
    pub pid: u32,
    pub name: String,
    pub cpu: Option<f32>,
    pub memory: Option<u64>,
    pub status: String,
    pub user: String,
    pub executable: String,
    pub virtual_memory: Option<u64>,
    pub elapsed: Option<u64>,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct Snapshot {
    pub sequence: u64,
    pub host: String,
    pub os: String,
    pub cpu_name: String,
    pub cpu_usage: f32,
    pub cores: Vec<f32>,
    pub memory_total: u64,
    pub memory_used: u64,
    pub memory_available: u64,
    pub swap_total: u64,
    pub swap_used: u64,
    pub memory_detail: MemoryDetail,
    pub cpu_caches: String,
    pub gpus: Vec<Gpu>,
    pub processes: Vec<Process>,
    pub load: [f64; 3],
    pub uptime: Option<u64>,
    pub collection_ms: f64,
    pub cpu_history: VecDeque<u64>,
    pub memory_history: VecDeque<u64>,
    #[serde(skip)]
    pub collected_at: Option<Instant>,
}

pub struct Collector {
    system: System,
    snapshot: Snapshot,
    users: Users,
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
        Self {
            system,
            snapshot,
            users: Users::new_with_refreshed_list(),
        }
    }

    pub fn sample(&mut self) -> Snapshot {
        let start = Instant::now();
        self.system.refresh_cpu_usage();
        self.system.refresh_memory();
        self.system
            .refresh_processes_specifics(ProcessesToUpdate::All, true, process_refresh());
        let s = &mut self.snapshot;
        s.sequence += 1;
        s.cpu_usage = self.system.global_cpu_usage();
        s.cores = self.system.cpus().iter().map(|c| c.cpu_usage()).collect();
        s.memory_total = self.system.total_memory();
        s.memory_used = self.system.used_memory();
        s.memory_available = self.system.available_memory();
        s.swap_total = self.system.total_swap();
        s.swap_used = self.system.used_swap();
        s.memory_detail = platform::memory();
        s.gpus = platform::gpus();
        s.processes = self
            .system
            .processes()
            .iter()
            .map(|(pid, p)| Process {
                pid: pid.as_u32(),
                name: p
                    .name()
                    .to_string_lossy()
                    .chars()
                    .filter(|c| !c.is_control())
                    .collect(),
                cpu: (!matches!(p.status(), sysinfo::ProcessStatus::Unknown(_)))
                    .then(|| p.cpu_usage()),
                memory: (!matches!(p.status(), sysinfo::ProcessStatus::Unknown(_)))
                    .then(|| p.memory()),
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
                virtual_memory: (!matches!(p.status(), sysinfo::ProcessStatus::Unknown(_)))
                    .then(|| p.virtual_memory()),
                elapsed: (p.start_time() > 0).then(|| p.run_time()),
            })
            .collect();
        let load = System::load_average();
        s.load = [load.one, load.five, load.fifteen];
        // sysinfo uses zero on a failed boot-time query, which would otherwise
        // make uptime look like the number of seconds since the Unix epoch.
        s.uptime = (System::boot_time() > 0).then(System::uptime);
        push_history(
            &mut s.cpu_history,
            s.cpu_usage.clamp(0.0, 100.0).round() as u64,
        );
        push_history(
            &mut s.memory_history,
            (ratio(s.memory_used, s.memory_total) * 100.0).round() as u64,
        );
        s.collection_ms = start.elapsed().as_secs_f64() * 1000.0;
        s.collected_at = Some(Instant::now());
        s.clone()
    }
}

fn process_refresh() -> ProcessRefreshKind {
    ProcessRefreshKind::nothing()
        .with_cpu()
        .with_memory()
        .with_user(UpdateKind::OnlyIfNotSet)
        .with_exe(UpdateKind::OnlyIfNotSet)
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
    fn units_and_ratios_are_safe() {
        assert_eq!(bytes(1073741824), "1.0 GiB");
        assert_eq!(ratio(1, 0), 0.0);
        assert_eq!(ratio(20, 10), 1.0);
        assert_eq!(ratio(1, 4), 0.25);
    }
}
