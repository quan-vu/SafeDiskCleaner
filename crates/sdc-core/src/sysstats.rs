//! Live system disk statistics for the "Disk Activity" page (read-only).

use core_foundation_sys::base::{kCFAllocatorDefault, CFRelease, CFTypeRef};
use core_foundation_sys::dictionary::{CFDictionaryGetValue, CFDictionaryRef};
use core_foundation_sys::number::{kCFNumberSInt64Type, CFNumberGetValue, CFNumberRef};
use core_foundation_sys::string::{kCFStringEncodingUTF8, CFStringCreateWithCString, CFStringRef};
use std::collections::{HashMap, VecDeque};
use std::ffi::{c_char, c_void, CStr, CString};
use std::path::Path;
use std::time::Instant;

// ---------- IOKit block-storage counters ----------

type IoObject = u32;
#[link(name = "IOKit", kind = "framework")]
extern "C" {
    fn IOServiceMatching(name: *const c_char) -> *mut c_void;
    fn IOServiceGetMatchingServices(main_port: u32, matching: *mut c_void, existing: *mut IoObject) -> i32;
    fn IOIteratorNext(iterator: IoObject) -> IoObject;
    fn IOObjectRelease(object: IoObject) -> i32;
    fn IORegistryEntryCreateCFProperty(entry: IoObject, key: CFStringRef, alloc: *const c_void, options: u32) -> CFTypeRef;
}

fn cfstr(s: &str) -> CFStringRef {
    let c = CString::new(s).unwrap();
    unsafe { CFStringCreateWithCString(kCFAllocatorDefault, c.as_ptr(), kCFStringEncodingUTF8) }
}

fn dict_i64(dict: CFDictionaryRef, key: &str) -> i64 {
    let k = cfstr(key);
    let v = unsafe { CFDictionaryGetValue(dict, k as *const c_void) } as CFNumberRef;
    let mut out: i64 = 0;
    if !v.is_null() {
        unsafe { CFNumberGetValue(v, kCFNumberSInt64Type, &mut out as *mut i64 as *mut c_void) };
    }
    unsafe { CFRelease(k as CFTypeRef) };
    out
}

/// Total bytes (read, written) by all physical disks since boot.
pub fn disk_io_totals() -> (u64, u64) {
    let (mut read, mut written) = (0u64, 0u64);
    unsafe {
        let matching = IOServiceMatching(c"IOBlockStorageDriver".as_ptr());
        let mut iter: IoObject = 0;
        if IOServiceGetMatchingServices(0, matching, &mut iter) != 0 {
            return (0, 0);
        }
        let key = cfstr("Statistics");
        loop {
            let entry = IOIteratorNext(iter);
            if entry == 0 {
                break;
            }
            let stats = IORegistryEntryCreateCFProperty(entry, key, kCFAllocatorDefault, 0);
            if !stats.is_null() {
                read += dict_i64(stats as CFDictionaryRef, "Bytes (Read)").max(0) as u64;
                written += dict_i64(stats as CFDictionaryRef, "Bytes (Write)").max(0) as u64;
                CFRelease(stats);
            }
            IOObjectRelease(entry);
        }
        CFRelease(key as CFTypeRef);
        IOObjectRelease(iter);
    }
    (read, written)
}

/// One-second throughput sample, bytes/s.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Sample {
    pub read: f64,
    pub write: f64,
}

/// Turns monotonically growing counters into per-second rates, keeping up to
/// `capacity` samples (3600 = one hour at 1 Hz).
pub struct RateHistory {
    last: Option<(u64, u64, Instant)>,
    pub samples: VecDeque<Sample>,
    pub capacity: usize,
    pub total_read: u64,
    pub total_written: u64,
}

impl RateHistory {
    pub fn new(capacity: usize) -> Self {
        RateHistory { last: None, samples: VecDeque::with_capacity(capacity), capacity, total_read: 0, total_written: 0 }
    }

    pub fn push(&mut self, read: u64, written: u64, now: Instant) {
        self.total_read = read;
        self.total_written = written;
        if let Some((r0, w0, t0)) = self.last {
            let dt = now.duration_since(t0).as_secs_f64().max(0.001);
            let s = Sample {
                read: read.saturating_sub(r0) as f64 / dt,
                write: written.saturating_sub(w0) as f64 / dt,
            };
            if self.samples.len() == self.capacity {
                self.samples.pop_front();
            }
            self.samples.push_back(s);
        }
        self.last = Some((read, written, now));
    }

    pub fn latest(&self) -> Sample {
        self.samples.back().copied().unwrap_or_default()
    }

    /// Last `secs` seconds folded into `buckets` columns (averaged), oldest first.
    /// Missing history is zero-filled on the left.
    pub fn buckets(&self, secs: usize, buckets: usize) -> Vec<Sample> {
        let mut window: Vec<Sample> = self.samples.iter().rev().take(secs).copied().collect();
        window.reverse();
        let pad = secs.saturating_sub(window.len());
        let mut full = vec![Sample::default(); pad];
        full.extend(window);
        // Fractional bucket width: when there are more columns than seconds,
        // a sample spans several columns instead of leaving the right side empty.
        let per = secs as f64 / buckets.max(1) as f64;
        let len = full.len();
        (0..buckets)
            .map(|b| {
                let lo = ((b as f64 * per).floor() as usize).min(len.saturating_sub(1));
                let hi = (((b + 1) as f64 * per).floor() as usize).clamp(lo + 1, len.max(lo + 1));
                let slice = &full[lo..hi.min(len)];
                let n = slice.len().max(1) as f64;
                Sample {
                    read: slice.iter().map(|s| s.read).sum::<f64>() / n,
                    write: slice.iter().map(|s| s.write).sum::<f64>() / n,
                }
            })
            .collect()
    }
}

// ---------- per-process disk I/O ----------

#[derive(Debug, Clone, PartialEq)]
pub struct ProcIo {
    pub name: String,
    /// Bundle path when the process belongs to an `.app` (for its icon).
    pub app_path: Option<String>,
    pub processes: usize,
    pub rate: f64,
}

fn pid_path(pid: i32) -> Option<String> {
    let mut buf = vec![0u8; 4096];
    let n = unsafe { libc::proc_pidpath(pid, buf.as_mut_ptr() as *mut c_void, buf.len() as u32) };
    if n <= 0 {
        return None;
    }
    buf.truncate(n as usize);
    String::from_utf8(buf).ok()
}

fn pid_name(pid: i32) -> String {
    let mut buf = [0u8; 256];
    let n = unsafe { libc::proc_name(pid, buf.as_mut_ptr() as *mut c_void, buf.len() as u32) };
    if n <= 0 {
        return format!("pid {pid}");
    }
    String::from_utf8_lossy(&buf[..n as usize]).into_owned()
}

/// Group key: the outermost `.app` bundle in the executable path, else the process name.
pub fn app_group(path: Option<&str>, name: &str) -> (String, Option<String>) {
    if let Some(p) = path {
        if let Some(idx) = p.find(".app/") {
            let bundle = &p[..idx + 4];
            let label = Path::new(bundle).file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
            return (label, Some(bundle.to_string()));
        }
    }
    (name.to_string(), None)
}

#[derive(Default)]
pub struct ProcSampler {
    last: HashMap<i32, (u64, String, Option<String>)>,
    last_at: Option<Instant>,
}

impl ProcSampler {
    /// Returns per-app read+write rates since the previous call, highest first.
    /// Processes of other users are skipped by the kernel (permission denied).
    pub fn sample(&mut self) -> Vec<ProcIo> {
        let now = Instant::now();
        let mut pids = vec![0i32; 8192];
        let n = unsafe { libc::proc_listallpids(pids.as_mut_ptr() as *mut c_void, (pids.len() * 4) as i32) };
        pids.truncate(n.max(0) as usize);
        let mut current = HashMap::with_capacity(pids.len());
        for pid in pids {
            if pid <= 0 {
                continue;
            }
            let mut info: libc::rusage_info_v4 = unsafe { std::mem::zeroed() };
            let rc = unsafe {
                libc::proc_pid_rusage(pid, libc::RUSAGE_INFO_V4, &mut info as *mut _ as *mut libc::rusage_info_t)
            };
            if rc != 0 {
                continue;
            }
            let bytes = info.ri_diskio_bytesread + info.ri_diskio_byteswritten;
            let (name, app) = match self.last.get(&pid) {
                Some((_, n, a)) => (n.clone(), a.clone()),
                None => app_group(pid_path(pid).as_deref(), &pid_name(pid)),
            };
            current.insert(pid, (bytes, name, app));
        }
        let mut out: HashMap<String, ProcIo> = HashMap::new();
        if let Some(t0) = self.last_at {
            let dt = now.duration_since(t0).as_secs_f64().max(0.001);
            for (pid, (bytes, name, app)) in &current {
                let delta = self.last.get(pid).map(|(b, _, _)| bytes.saturating_sub(*b)).unwrap_or(0);
                let e = out.entry(name.clone()).or_insert_with(|| ProcIo {
                    name: name.clone(),
                    app_path: app.clone(),
                    processes: 0,
                    rate: 0.0,
                });
                e.processes += 1;
                e.rate += delta as f64 / dt;
            }
        }
        self.last = current;
        self.last_at = Some(now);
        let mut list: Vec<ProcIo> = out.into_values().filter(|p| p.rate > 0.0).collect();
        list.sort_by(|a, b| b.rate.partial_cmp(&a.rate).unwrap_or(std::cmp::Ordering::Equal));
        list
    }
}

// ---------- mounts and disk images ----------

#[derive(Debug, Clone, PartialEq)]
pub struct Mount {
    pub mount_point: String,
    pub device: String,
    pub fs_type: String,
    pub total: i64,
    pub free: i64,
}

fn cs(buf: &[c_char]) -> String {
    unsafe { CStr::from_ptr(buf.as_ptr()) }.to_string_lossy().into_owned()
}

/// User-visible local mounts (what Finder shows), excluding system-only volumes.
pub fn mounts() -> Vec<Mount> {
    let mut ptr: *mut libc::statfs = std::ptr::null_mut();
    let n = unsafe { libc::getmntinfo(&mut ptr, libc::MNT_NOWAIT) };
    if n <= 0 || ptr.is_null() {
        return vec![];
    }
    let all = unsafe { std::slice::from_raw_parts(ptr, n as usize) };
    all.iter()
        .filter(|st| {
            let flags = st.f_flags as i32;
            let mp = cs(&st.f_mntonname);
            flags & libc::MNT_LOCAL != 0
                && (flags & libc::MNT_DONTBROWSE == 0 || mp == "/System/Volumes/Data")
                && mp != "/"
                && st.f_blocks > 0
        })
        .map(|st| Mount {
            mount_point: cs(&st.f_mntonname),
            device: cs(&st.f_mntfromname).trim_start_matches("/dev/").to_string(),
            fs_type: cs(&st.f_fstypename),
            total: st.f_blocks as i64 * st.f_bsize as i64,
            free: st.f_bavail as i64 * st.f_bsize as i64,
        })
        .collect()
}

#[derive(Debug, Clone, PartialEq)]
pub struct DiskImage {
    pub name: String,
    pub image_path: String,
    pub mount_point: Option<String>,
    pub size: u64,
    pub fs_type: String,
}

/// Parse `hdiutil info -plist` output.
pub fn parse_hdiutil(xml: &[u8]) -> Vec<DiskImage> {
    let Ok(v) = plist::Value::from_reader(std::io::Cursor::new(xml)) else { return vec![] };
    let Some(images) = v.as_dictionary().and_then(|d| d.get("images")).and_then(|i| i.as_array()) else {
        return vec![];
    };
    images
        .iter()
        .filter_map(|img| {
            let d = img.as_dictionary()?;
            let image_path = d.get("image-path")?.as_string()?.to_string();
            let entities = d.get("system-entities").and_then(|e| e.as_array()).cloned().unwrap_or_default();
            let mut mount_point = None;
            let mut fs_type = String::new();
            for e in entities.iter().filter_map(|e| e.as_dictionary()) {
                if let Some(mp) = e.get("mount-point").and_then(|m| m.as_string()) {
                    mount_point = Some(mp.to_string());
                    let hint = e.get("content-hint").and_then(|h| h.as_string()).unwrap_or("");
                    let h = hint.to_uppercase();
                    fs_type = if h.contains("HFS") || h.starts_with("48465300") {
                        "hfs".into()
                    } else if h.contains("APFS") || h.starts_with("7C3457EF") {
                        "apfs".into()
                    } else {
                        hint.to_lowercase()
                    };
                }
            }
            let name = mount_point
                .as_deref()
                .and_then(|m| Path::new(m).file_name())
                .or_else(|| Path::new(&image_path).file_stem())
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default();
            let size = std::fs::metadata(&image_path).map(|m| m.len()).unwrap_or(0);
            Some(DiskImage { name, image_path, mount_point, size, fs_type })
        })
        .collect()
}

pub fn disk_images() -> Vec<DiskImage> {
    let mut cmd = std::process::Command::new("/usr/bin/hdiutil");
    cmd.args(["info", "-plist"]);
    let mut images = match crate::proc::run(cmd, None, std::time::Duration::from_secs(5)) {
        Ok(out) if out.status.success() => parse_hdiutil(&out.stdout),
        _ => vec![],
    };
    // Prefer the mounted volume's own file system type and capacity.
    for img in images.iter_mut() {
        if let Some((fs, total)) = img.mount_point.as_deref().and_then(statfs_info) {
            img.fs_type = fs;
            img.size = total;
        }
    }
    images
}

fn statfs_info(path: &str) -> Option<(String, u64)> {
    let c = CString::new(path).ok()?;
    let mut st: libc::statfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statfs(c.as_ptr(), &mut st) } != 0 {
        return None;
    }
    Some((cs(&st.f_fstypename), st.f_blocks * st.f_bsize as u64))
}

/// Marketing model name, e.g. "MacBook Air" (slow: call once, off the main thread).
pub fn machine_name() -> String {
    let mut cmd = std::process::Command::new("/usr/sbin/system_profiler");
    cmd.args(["SPHardwareDataType", "-json"]);
    if let Ok(out) = crate::proc::run(cmd, None, std::time::Duration::from_secs(10)) {
        if let Ok(v) = serde_json::from_slice::<serde_json::Value>(&out.stdout) {
            if let Some(name) = v["SPHardwareDataType"][0]["machine_name"].as_str() {
                return name.to_string();
            }
        }
    }
    "Mac".into()
}

// ---------- CPU, memory, temporary files ----------

/// Total CPU load across all cores, from the difference between two samples.
#[derive(Default)]
pub struct CpuSampler {
    prev: Option<(u64, u64)>,
}

#[allow(deprecated)] // libc points to the mach2 crate; these calls are stable.
fn cpu_ticks() -> Option<(u64, u64)> {
    let mut count: libc::natural_t = 0;
    let mut info: libc::processor_info_array_t = std::ptr::null_mut();
    let mut info_count: libc::mach_msg_type_number_t = 0;
    let kr = unsafe {
        libc::host_processor_info(
            libc::mach_host_self(),
            libc::PROCESSOR_CPU_LOAD_INFO,
            &mut count,
            &mut info,
            &mut info_count,
        )
    };
    if kr != 0 || info.is_null() {
        return None;
    }
    let states = libc::CPU_STATE_MAX as usize;
    let (mut busy, mut total) = (0u64, 0u64);
    for cpu in 0..count as usize {
        for state in 0..states {
            let v = unsafe { *info.add(cpu * states + state) } as u32 as u64;
            total += v;
            if state != libc::CPU_STATE_IDLE as usize {
                busy += v;
            }
        }
    }
    unsafe {
        libc::vm_deallocate(
            libc::mach_task_self(),
            info as libc::vm_address_t,
            (info_count as usize * std::mem::size_of::<libc::integer_t>()) as libc::vm_size_t,
        );
    }
    Some((busy, total))
}

impl CpuSampler {
    /// Load in percent (0–100) since the previous call; `None` on the first call.
    pub fn sample(&mut self) -> Option<f64> {
        let now = cpu_ticks()?;
        let prev = self.prev.replace(now)?;
        let total = now.1.saturating_sub(prev.1);
        if total == 0 {
            return Some(0.0);
        }
        Some(now.0.saturating_sub(prev.0) as f64 * 100.0 / total as f64)
    }
}

fn sysctl_u64(name: &str) -> Option<u64> {
    let c = CString::new(name).ok()?;
    let mut v: u64 = 0;
    let mut len = std::mem::size_of::<u64>();
    let r = unsafe { libc::sysctlbyname(c.as_ptr(), &mut v as *mut u64 as *mut c_void, &mut len, std::ptr::null_mut(), 0) };
    (r == 0).then_some(if len == 4 { v & 0xffff_ffff } else { v })
}

pub fn cpu_cores() -> usize {
    sysctl_u64("hw.logicalcpu").unwrap_or(1) as usize
}

/// Memory split like Activity Monitor (bytes).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct MemInfo {
    pub total: u64,
    pub app: u64,
    pub wired: u64,
    pub compressed: u64,
}

impl MemInfo {
    pub fn used(&self) -> u64 {
        (self.app + self.wired + self.compressed).min(self.total)
    }
}

#[allow(deprecated)]
pub fn memory() -> Option<MemInfo> {
    let total = sysctl_u64("hw.memsize")?;
    let page = sysctl_u64("vm.pagesize").unwrap_or(16384);
    let mut st: libc::vm_statistics64 = unsafe { std::mem::zeroed() };
    let mut count = libc::HOST_VM_INFO64_COUNT;
    let kr = unsafe {
        libc::host_statistics64(libc::mach_host_self(), libc::HOST_VM_INFO64, &mut st as *mut _ as libc::host_info64_t, &mut count)
    };
    if kr != 0 {
        return None;
    }
    let app = (st.internal_page_count as u64).saturating_sub(st.purgeable_count as u64) * page;
    Some(MemInfo { total, app, wired: st.wire_count as u64 * page, compressed: st.compressor_page_count as u64 * page })
}

/// On-disk size of the usual temporary areas in the home folder (bytes).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct TempSizes {
    pub caches: i64,
    pub logs: i64,
    pub trash: i64,
}

impl TempSizes {
    pub fn total(&self) -> i64 {
        self.caches + self.logs + self.trash
    }
}

/// Sum of allocated blocks under `dir` (no symlinks, same volume, unreadable parts skipped).
pub fn dir_size(dir: &Path) -> i64 {
    use std::os::unix::fs::MetadataExt;
    let Ok(root) = std::fs::symlink_metadata(dir) else { return 0 };
    let dev = root.dev();
    let mut total = 0i64;
    let mut it = walkdir::WalkDir::new(dir).follow_links(false).into_iter();
    while let Some(entry) = it.next() {
        let Ok(entry) = entry else { continue };
        let Ok(md) = entry.metadata() else { continue };
        if md.dev() != dev {
            if md.is_dir() {
                it.skip_current_dir();
            }
            continue;
        }
        if md.is_file() {
            total += md.blocks() as i64 * 512;
        }
    }
    total
}

/// Slow (walks the folders): call off the main thread.
pub fn temp_sizes(home: &Path) -> TempSizes {
    TempSizes {
        caches: dir_size(&home.join("Library/Caches")),
        logs: dir_size(&home.join("Library/Logs")),
        trash: dir_size(&home.join(".Trash")),
    }
}
