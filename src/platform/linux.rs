use super::{Gpu, MemoryDetail};
use std::{
    collections::BTreeSet,
    fs,
    io::Read,
    path::Path,
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

pub fn memory() -> MemoryDetail {
    fs::read_to_string("/proc/meminfo")
        .map(|s| parse_meminfo(&s))
        .unwrap_or_default()
}

fn parse_meminfo(text: &str) -> MemoryDetail {
    let field = |key: &str| -> Option<u64> {
        text.lines().find_map(|line| {
            let (name, value) = line.split_once(':')?;
            (name == key)
                .then(|| {
                    value
                        .split_whitespace()
                        .next()?
                        .parse::<u64>()
                        .ok()?
                        .checked_mul(1024)
                })
                .flatten()
        })
    };
    MemoryDetail {
        cache_bytes: field("Cached")
            .zip(field("SReclaimable"))
            .zip(field("Shmem"))
            .map(|((cached, reclaimable), shared)| {
                cached.saturating_add(reclaimable).saturating_sub(shared)
            }),
        compressed_bytes: None,
        label: "Page cache + reclaimable".into(),
        ..Default::default()
    }
}

fn read_number(path: impl AsRef<Path>) -> Option<u64> {
    fs::read_to_string(path).ok()?.trim().parse().ok()
}

pub fn gpus() -> Vec<Gpu> {
    let mut result = Vec::new();
    if let Ok(entries) = fs::read_dir("/sys/class/drm") {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if !name
                .strip_prefix("card")
                .is_some_and(|n| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()))
            {
                continue;
            }
            let device = entry.path().join("device");
            let vendor = fs::read_to_string(device.join("vendor")).unwrap_or_default();
            if vendor.trim() == "0x10de" {
                continue;
            } // NVIDIA collected below.
            let utilization = read_number(device.join("gpu_busy_percent"))
                .filter(|v| *v <= 100)
                .map(|v| v as f64);
            let memory_used = read_number(device.join("mem_info_vram_used"));
            let memory_total = read_number(device.join("mem_info_vram_total"));
            let temperature = fs::read_dir(device.join("hwmon")).ok().and_then(|entries| {
                entries
                    .flatten()
                    .find_map(|e| read_number(e.path().join("temp1_input")))
                    .map(|v| v as f64 / 1000.0)
            });
            let vendor = match vendor.trim() {
                "0x1002" => "AMD",
                "0x8086" => "Intel",
                _ => "GPU",
            };
            result.push(Gpu {
                name: format!("{vendor} {name}"),
                utilization,
                memory_used,
                memory_total,
                temperature,
                source: "DRM sysfs".into(),
            });
        }
    }
    if let Some(output) = nvidia_output() {
        result.extend(parse_nvidia(&output));
    }
    result
}

// A driver utility must never freeze the sampling worker. Output is tiny and
// read concurrently so even unexpected output cannot fill the child's pipe.
fn nvidia_output() -> Option<String> {
    let mut child = Command::new("nvidia-smi")
        .args([
            "--query-gpu=name,utilization.gpu,memory.used,memory.total,temperature.gpu",
            "--format=csv,noheader,nounits",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let stdout = child.stdout.take()?;
    let reader = thread::spawn(move || {
        let mut output = String::new();
        stdout
            .take(64 * 1024)
            .read_to_string(&mut output)
            .ok()
            .map(|_| output)
    });
    let deadline = Instant::now() + Duration::from_millis(700);
    let success = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status.success(),
            Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(10)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                break false;
            }
        }
    };
    let output = reader.join().ok().flatten();
    if success {
        output
    } else {
        None
    }
}

fn parse_nvidia(text: &str) -> Vec<Gpu> {
    text.lines()
        .filter_map(|line| {
            let fields: Vec<_> = line.split(',').map(str::trim).collect();
            if fields.len() != 5 {
                return None;
            }
            let value = |i: usize| {
                fields[i]
                    .parse::<f64>()
                    .ok()
                    .filter(|v| v.is_finite() && *v >= 0.0)
            };
            Some(Gpu {
                name: fields[0].into(),
                utilization: value(1).filter(|v| *v <= 100.0),
                memory_used: value(2).map(|v| (v * 1048576.0) as u64),
                memory_total: value(3).map(|v| (v * 1048576.0) as u64),
                temperature: value(4),
                source: "nvidia-smi".into(),
            })
        })
        .collect()
}

pub fn cpu_caches() -> String {
    let mut caches = BTreeSet::new();
    if let Ok(entries) = fs::read_dir("/sys/devices/system/cpu/cpu0/cache") {
        for entry in entries.flatten() {
            let path = entry.path();
            if let (Ok(level), Ok(kind), Ok(size)) = (
                fs::read_to_string(path.join("level")),
                fs::read_to_string(path.join("type")),
                fs::read_to_string(path.join("size")),
            ) {
                let suffix = match kind.trim() {
                    "Data" => "d",
                    "Instruction" => "i",
                    _ => "",
                };
                caches.insert(format!("L{}{suffix} {}", level.trim(), size.trim()));
            }
        }
    }
    caches.into_iter().collect::<Vec<_>>().join("   ")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cache_excludes_shared_memory() {
        let m = parse_meminfo("Cached: 100 kB\nSReclaimable: 20 kB\nShmem: 30 kB\n");
        assert_eq!(m.cache_bytes, Some(90 * 1024));
        assert_eq!(parse_meminfo("Cached: 10 kB").cache_bytes, None);
    }
    #[test]
    fn gpu_unavailable_is_not_zero() {
        let g = parse_nvidia("RTX 4090, [N/A], 1024, 24576, 42\n");
        assert_eq!(g[0].utilization, None);
        assert_eq!(g[0].memory_used, Some(1073741824));
        assert!(parse_nvidia("bad output").is_empty());
    }
}
