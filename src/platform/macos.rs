use super::{Gpu, MemoryDetail};
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
                .filter(|v| *v >= 0.0)
                .map(|v| v as u64),
            memory_total: None, // Unified memory is not dedicated VRAM.
            temperature: None,
            source: "IORegistry · shared memory".into(),
        });
    }
    result
}

pub fn memory() -> MemoryDetail {
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
        if ok && page_ok {
            result.cache_bytes = Some(u64::from(stats.external_page_count) * page_size as u64);
            result.compressed_bytes =
                Some(u64::from(stats.compressor_page_count) * page_size as u64);
        }
    }
    result
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
