use super::{Gpu, MemoryDetail, MemoryReading};
use core_foundation::{
    base::{CFType, TCFType},
    dictionary::CFDictionary,
    number::CFNumber,
    string::CFString,
};
use std::{
    ffi::{c_char, c_void, CString},
    mem, ptr,
};

// These libSystem functions are not all exposed by libc's Rust bindings.
extern "C" {
    fn mach_host_self() -> libc::mach_port_t;
    fn host_page_size(host: libc::mach_port_t, size: *mut libc::vm_size_t) -> libc::kern_return_t;
    fn mach_port_deallocate(
        task: libc::mach_port_t,
        name: libc::mach_port_t,
    ) -> libc::kern_return_t;
}

#[link(name = "IOKit", kind = "framework")]
extern "C" {
    fn IOServiceMatching(name: *const c_char) -> *mut c_void;
    fn IOServiceGetMatchingServices(port: u32, matching: *mut c_void, iterator: *mut u32) -> i32;
    fn IOIteratorNext(iterator: u32) -> u32;
    fn IOObjectRelease(object: u32) -> i32;
    fn IORegistryEntryCreateCFProperty(
        entry: u32,
        key: *const c_void,
        allocator: *const c_void,
        options: u32,
    ) -> *const c_void;
}

struct IoObject(u32);
impl Drop for IoObject {
    fn drop(&mut self) {
        // SAFETY: this wrapper owns one IOKit reference returned by a successful call.
        unsafe {
            IOObjectRelease(self.0);
        }
    }
}

fn property(entry: u32, key: &str) -> Option<CFType> {
    let key = CFString::new(key);
    // SAFETY: entry remains alive for this call; the key is a valid CFString.
    let value =
        unsafe { IORegistryEntryCreateCFProperty(entry, key.as_CFTypeRef(), ptr::null(), 0) };
    if value.is_null() {
        None
    } else {
        // SAFETY: Create returns an owned Core Foundation reference.
        Some(unsafe { CFType::wrap_under_create_rule(value) })
    }
}

pub fn gpus() -> Vec<Gpu> {
    let mut result = Vec::new();
    // SAFETY: constant NUL-terminated class name; matching is consumed by IOKit.
    let matching = unsafe { IOServiceMatching(c"IOAccelerator".as_ptr()) };
    if matching.is_null() {
        return result;
    }
    let mut iterator = 0;
    if unsafe { IOServiceGetMatchingServices(0, matching, &mut iterator) } != 0 {
        return result;
    }
    let iterator = IoObject(iterator);
    loop {
        let entry = unsafe { IOIteratorNext(iterator.0) };
        if entry == 0 {
            break;
        }
        let entry = IoObject(entry);
        let stats =
            property(entry.0, "PerformanceStatistics").and_then(|v| v.downcast::<CFDictionary>());
        let number = |key: &str| -> Option<f64> {
            let dict = stats.as_ref()?;
            let key = CFString::new(key);
            let value = dict.find(key.as_CFTypeRef())?;
            // SAFETY: dictionary retains this value; wrapping follows the Get rule.
            let value = unsafe { CFType::wrap_under_get_rule(*value) };
            value.downcast::<CFNumber>()?.to_f64()
        };
        let name = property(entry.0, "model")
            .and_then(|v| v.downcast::<CFString>())
            .map(|v| v.to_string())
            .unwrap_or_else(|| "Apple / integrated GPU".into());
        result.push(Gpu {
            name,
            utilization: number("Device Utilization %").filter(|v| (0.0..=100.0).contains(v)),
            memory_used: number("In use system memory")
                .filter(|v| v.is_finite() && *v >= 0.0 && *v < u64::MAX as f64)
                .map(|v| v as u64),
            memory_total: None, // Unified memory is not dedicated VRAM.
            temperature: None,
            source: "IORegistry · shared memory".into(),
        });
    }
    result
}

pub fn memory() -> (MemoryReading, MemoryDetail) {
    let mut reading = MemoryReading {
        total: sysctl_u64("hw.memsize"),
        ..Default::default()
    };
    let mut swap: libc::xsw_usage = unsafe { mem::zeroed() };
    let mut size = mem::size_of_val(&swap);
    // SAFETY: read-only sysctl with a correctly sized C output structure.
    if unsafe {
        libc::sysctlbyname(
            c"vm.swapusage".as_ptr(),
            &mut swap as *mut _ as *mut c_void,
            &mut size,
            ptr::null_mut(),
            0,
        )
    } == 0
        && size == mem::size_of_val(&swap)
    {
        reading.swap_total = Some(swap.xsu_total);
        reading.swap_used = Some(swap.xsu_used);
    }
    let mut result = MemoryDetail {
        label: "File-backed".into(),
        ..Default::default()
    };
    // SAFETY: zero is valid for this C statistics structure. The kernel receives
    // its exact size in integer_t units and the host send right is released.
    unsafe {
        let mut stats: libc::vm_statistics64 = mem::zeroed();
        let mut count = (mem::size_of_val(&stats) / mem::size_of::<libc::integer_t>()) as u32;
        let host = mach_host_self();
        let mut page_size = 0;
        let page_ok = host_page_size(host, &mut page_size) == 0;
        let ok = libc::host_statistics64(
            host,
            libc::HOST_VM_INFO64,
            &mut stats as *mut _ as *mut _,
            &mut count,
        ) == 0;
        #[allow(deprecated)]
        mach_port_deallocate(libc::mach_task_self(), host);
        if ok && vm_fields_available(count) && page_ok && page_size > 0 {
            // Page categories do not cover all physical RAM (e.g. reserved
            // kernel/driver allocations). Keep that remainder in used memory.
            let accounted = used_memory_pages(
                u64::from(stats.internal_page_count),
                u64::from(stats.purgeable_count),
                u64::from(stats.wire_count),
                u64::from(stats.compressor_page_count),
            ) * page_size as u64;
            let free = u64::from(stats.free_count) * page_size as u64;
            let cached = (u64::from(stats.external_page_count) + u64::from(stats.purgeable_count))
                * page_size as u64;
            result.free_bytes = Some(free);
            if let Some((used, available)) = reading
                .total
                .and_then(|total| physical_memory(total, free, cached))
            {
                reading.used = Some(used);
                reading.available = Some(available);
                result.other_bytes = used.checked_sub(accounted);
            }
            result.app_bytes = Some(
                u64::from(stats.internal_page_count)
                    .saturating_sub(u64::from(stats.purgeable_count))
                    * page_size as u64,
            );
            result.wired_bytes = Some(u64::from(stats.wire_count) * page_size as u64);
            result.cached_files_bytes = Some(
                (u64::from(stats.external_page_count) + u64::from(stats.purgeable_count))
                    * page_size as u64,
            );
            result.cache_bytes = Some(u64::from(stats.external_page_count) * page_size as u64);
            result.compressed_bytes =
                Some(u64::from(stats.compressor_page_count) * page_size as u64);
        }
    }
    (reading, result)
}

fn physical_memory(total: u64, free: u64, cached: u64) -> Option<(u64, u64)> {
    let available = free.checked_add(cached)?;
    Some((total.checked_sub(available)?, available))
}

fn vm_fields_available(count: u32) -> bool {
    // Older macOS kernels return a shorter revision than current libc defines.
    // Require only the fields we actually read, through internal_page_count.
    count as usize * mem::size_of::<libc::integer_t>()
        >= mem::offset_of!(libc::vm_statistics64, internal_page_count)
            + mem::size_of::<libc::natural_t>()
}

fn used_memory_pages(anonymous: u64, purgeable: u64, wired: u64, compressed: u64) -> u64 {
    anonymous
        .saturating_sub(purgeable)
        .saturating_add(wired)
        .saturating_add(compressed)
}

fn sysctl_u64(name: &str) -> Option<u64> {
    let name = CString::new(name).ok()?;
    let mut value = 0u64;
    let mut size = mem::size_of::<u64>();
    // SAFETY: valid C name and correctly sized writable output; no new value.
    let status = unsafe {
        libc::sysctlbyname(
            name.as_ptr(),
            &mut value as *mut _ as *mut _,
            &mut size,
            ptr::null_mut(),
            0,
        )
    };
    (status == 0 && value > 0).then_some(value)
}

pub fn cpu_caches() -> String {
    [
        ("L1d", "hw.l1dcachesize"),
        ("L1i", "hw.l1icachesize"),
        ("L2", "hw.l2cachesize"),
        ("L3", "hw.l3cachesize"),
    ]
    .into_iter()
    .filter_map(|(label, key)| {
        sysctl_u64(key).map(|v| format!("{label} {}", crate::metrics::bytes(v)))
    })
    .collect::<Vec<_>>()
    .join("   ")
}

/// Read both identity and task counters in one kernel call. A denied/short read
/// must not become a valid zero, and PID reuse must not bridge CPU samples.
pub fn process_sample(pid: u32) -> Option<ProcessSample> {
    let pid = i32::try_from(pid).ok()?;
    // SAFETY: zero initializes this plain C output structure; proc_pidinfo is
    // given its exact writable size. Only a complete result is accepted.
    let mut info: libc::proc_taskallinfo = unsafe { mem::zeroed() };
    let size = mem::size_of_val(&info) as i32;
    let received = unsafe {
        libc::proc_pidinfo(
            pid,
            libc::PROC_PIDTASKALLINFO,
            0,
            &mut info as *mut _ as *mut c_void,
            size,
        )
    };
    if received != size || info.pbsd.pbi_pid != pid as u32 {
        return None;
    }
    #[allow(deprecated)] // libc still exposes the native Mach clock on macOS.
    let wall_ticks = unsafe { libc::mach_absolute_time() };
    // Rusage footprint is the ledger metric used by Activity Monitor. Accept
    // it only when the task identity is unchanged across the separate calls.
    let mut usage: libc::rusage_info_v0 = unsafe { mem::zeroed() };
    let footprint_ok = unsafe {
        libc::proc_pid_rusage(
            pid,
            libc::RUSAGE_INFO_V0,
            &mut usage as *mut _ as *mut libc::rusage_info_t,
        )
    } == 0;
    let mut identity_check: libc::proc_bsdinfo = unsafe { mem::zeroed() };
    let identity_size = mem::size_of_val(&identity_check) as i32;
    let identity_ok = unsafe {
        libc::proc_pidinfo(
            pid,
            libc::PROC_PIDTBSDINFO,
            0,
            &mut identity_check as *mut _ as *mut c_void,
            identity_size,
        )
    } == identity_size
        && identity_check.pbi_start_tvsec == info.pbsd.pbi_start_tvsec
        && identity_check.pbi_start_tvusec == info.pbsd.pbi_start_tvusec;
    Some(ProcessSample {
        footprint: (footprint_ok && identity_ok).then_some(usage.ri_phys_footprint),
        identity: (info.pbsd.pbi_start_tvsec, info.pbsd.pbi_start_tvusec),
        cpu_ticks: info
            .ptinfo
            .pti_total_user
            .checked_add(info.ptinfo.pti_total_system)?,
        // PROC_PIDTASKINFO times and mach_absolute_time share Mach time units.
        wall_ticks,
        resident: info.ptinfo.pti_resident_size,
        virtual_size: info.ptinfo.pti_virtual_size,
    })
}

#[derive(Clone, Copy)]
pub struct ProcessSample {
    pub identity: (u64, u64),
    pub cpu_ticks: u64,
    pub wall_ticks: u64,
    pub resident: u64,
    pub footprint: Option<u64>,
    pub virtual_size: u64,
}

impl ProcessSample {
    pub fn cpu_since(self, previous: Self) -> Option<f32> {
        if self.identity != previous.identity {
            return None;
        }
        let wall = self.wall_ticks.checked_sub(previous.wall_ticks)?;
        let cpu = self.cpu_ticks.checked_sub(previous.cpu_ticks)?;
        (wall > 0).then(|| (cpu as f64 / wall as f64 * 100.0) as f32)
    }
}

#[cfg(test)]
mod accuracy_tests {
    use super::*;
    #[test]
    fn process_cpu_uses_deltas_and_rejects_reused_pids() {
        let old = ProcessSample {
            identity: (10, 20),
            cpu_ticks: 100,
            wall_ticks: 1000,
            resident: 0,
            footprint: None,
            virtual_size: 0,
        };
        let new = ProcessSample {
            cpu_ticks: 400,
            wall_ticks: 1200,
            ..old
        };
        assert_eq!(new.cpu_since(old), Some(150.0));
        assert_eq!(old.cpu_since(old), None);
        assert_eq!(
            ProcessSample {
                identity: (10, 21),
                ..new
            }
            .cpu_since(old),
            None
        );
        assert_eq!(
            ProcessSample {
                cpu_ticks: 0,
                ..new
            }
            .cpu_since(old),
            None
        );
        assert_eq!(
            ProcessSample {
                cpu_ticks: 100,
                ..new
            }
            .cpu_since(old),
            Some(0.0)
        );
    }
    #[test]
    fn used_ram_includes_memory_outside_named_page_categories() {
        assert_eq!(physical_memory(8192, 100, 1092), Some((7000, 1192)));
        assert_eq!(physical_memory(100, 200, 0), None);
        assert_eq!(physical_memory(100, u64::MAX, 1), None);
    }
    #[test]
    fn older_vm_statistics_revision_has_all_required_fields() {
        assert!(vm_fields_available(38));
        assert!(!vm_fields_available(0));
        assert!(!vm_fields_available(20));
    }
    #[test]
    fn memory_excludes_reclaimable_pages_and_counts_compressor_once() {
        assert_eq!(used_memory_pages(100, 20, 30, 10), 120);
        assert_eq!(used_memory_pages(10, 20, 30, 10), 40);
    }
    #[test]
    fn cpu_ticks_distinguish_idle_missing_reset_and_wrap() {
        assert_eq!(
            cpu_deltas(&[[10, 0, 10, 0]], &[[20, 0, 20, 0]]),
            (Some(50.0), vec![Some(50.0)])
        );
        assert_eq!(
            cpu_deltas(&[[10, 0, 10, 0]], &[[10, 0, 20, 0]]),
            (Some(0.0), vec![Some(0.0)])
        );
        assert_eq!(
            cpu_deltas(&[[10, 0, 10, 0]], &[[10, 0, 10, 0]]),
            (None, vec![None])
        );
        assert_eq!(
            cpu_deltas(&[[100, 0, 100, 0]], &[[1, 0, 1, 0]]),
            (None, vec![None])
        );
        assert_eq!(
            cpu_deltas(&[[u32::MAX - 4, 0, 0, 0]], &[[5, 0, 10, 0]]),
            (Some(50.0), vec![Some(50.0)])
        );
    }
    #[test]
    fn inaccessible_process_does_not_become_zero() {
        assert!(process_sample(u32::MAX).is_none());
    }
}

/// Checked kernel ticks: failure invalidates the baseline, never reuses a value.
#[derive(Default)]
pub struct CpuSampler {
    previous: Option<Vec<[u32; 4]>>,
}

impl CpuSampler {
    pub fn sample(&mut self) -> (Option<f32>, Vec<Option<f32>>) {
        let current = cpu_ticks();
        let result = match (&self.previous, &current) {
            (Some(old), Some(new)) if old.len() == new.len() => cpu_deltas(old, new),
            (_, Some(new)) => (None, vec![None; new.len()]),
            _ => (None, Vec::new()),
        };
        self.previous = current;
        result
    }
}

fn cpu_deltas(old: &[[u32; 4]], new: &[[u32; 4]]) -> (Option<f32>, Vec<Option<f32>>) {
    let mut busy_total = 0u64;
    let mut ticks_total = 0u64;
    let cores: Vec<_> = old
        .iter()
        .zip(new)
        .map(|(old, new)| {
            let delta: Vec<u64> = new
                .iter()
                .zip(old)
                .map(|(n, o)| u64::from(n.wrapping_sub(*o)))
                .collect();
            // A reset is not a near-full 32-bit interval. Normal wrapping
            // counters produce a small positive delta across u32::MAX.
            if delta.iter().any(|v| *v > i32::MAX as u64) {
                return None;
            }
            let total: u64 = delta.iter().sum();
            let busy = total - delta[libc::CPU_STATE_IDLE as usize];
            busy_total += busy;
            ticks_total += total;
            (total > 0).then(|| (busy as f64 / total as f64 * 100.0) as f32)
        })
        .collect();
    let aggregate = (ticks_total > 0 && cores.iter().all(Option::is_some))
        .then(|| (busy_total as f64 / ticks_total as f64 * 100.0) as f32);
    (aggregate, cores)
}

fn cpu_ticks() -> Option<Vec<[u32; 4]>> {
    // SAFETY: IOKernel allocates the array; validate its length, copy it, then
    // release exactly the returned byte count and our owned host send right.
    #[allow(deprecated)]
    unsafe {
        let host = mach_host_self();
        let mut processors = 0;
        let mut data = ptr::null_mut();
        let mut count = 0;
        let status = libc::host_processor_info(
            host,
            libc::PROCESSOR_CPU_LOAD_INFO,
            &mut processors,
            &mut data,
            &mut count,
        );
        mach_port_deallocate(libc::mach_task_self(), host);
        if status != 0 {
            return None;
        }
        if data.is_null() {
            return None;
        }
        let result = if processors > 0 && count as usize == processors as usize * 4 {
            let slice = std::slice::from_raw_parts(data, count as usize);
            Some(
                slice
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .map(|v| [v[0] as u32, v[1] as u32, v[2] as u32, v[3] as u32])
                    .collect(),
            )
        } else {
            None
        };
        libc::vm_deallocate(
            libc::mach_task_self(),
            data as libc::vm_address_t,
            count as libc::vm_size_t * mem::size_of::<libc::integer_t>() as libc::vm_size_t,
        );
        result
    }
}

pub fn load_average() -> Option<[f64; 3]> {
    let mut values = [0.0; 3];
    // SAFETY: output has exactly the requested three doubles.
    let count = unsafe { libc::getloadavg(values.as_mut_ptr(), 3) };
    (count == 3 && values.iter().all(|v| v.is_finite() && *v >= 0.0)).then_some(values)
}
