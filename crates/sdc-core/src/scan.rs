//! Filesystem scan. Same skip rules as the Go `scan.go`.

use crate::model::{clean, level, protected, within, Candidate, Safety, Settings, Volume};
use chrono::{DateTime, Duration, TimeZone, Utc};
use std::collections::{HashMap, HashSet};
use std::ffi::CStr;
use std::os::unix::fs::MetadataExt;
use std::path::Path;
use std::process::Command;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::Mutex;
use std::time::{Duration as StdDuration, Instant};
use walkdir::WalkDir;

#[derive(Debug, Default)]
pub struct ScanResult {
    pub candidates: Vec<Candidate>,
    pub volumes: Vec<Volume>,
    pub skipped: usize,
    /// On-disk bytes per top-level child of each scan root (protected areas excluded).
    pub folder_sizes: Vec<(String, i64)>,
}

#[derive(Debug, thiserror::Error)]
pub enum ScanError {
    #[error("{0}")]
    Invalid(String),
    #[error("scan cancelled")]
    Cancelled,
}

/// Live scan progress, shared with the UI. Counters are updated as the scan runs;
/// `take_log` drains the log lines written since the last call.
pub struct Progress {
    pub dirs: AtomicU64,
    pub files: AtomicU64,
    pub found: AtomicUsize,
    pub skipped: AtomicUsize,
    pub lookups_done: AtomicUsize,
    pub lookups_total: AtomicUsize,
    phase: Mutex<String>,
    current: Mutex<(String, Instant)>,
    log: Mutex<Vec<String>>,
}

impl Default for Progress {
    fn default() -> Self {
        Progress {
            dirs: AtomicU64::new(0),
            files: AtomicU64::new(0),
            found: AtomicUsize::new(0),
            skipped: AtomicUsize::new(0),
            lookups_done: AtomicUsize::new(0),
            lookups_total: AtomicUsize::new(0),
            phase: Mutex::new(String::new()),
            current: Mutex::new((String::new(), Instant::now())),
            log: Mutex::new(Vec::new()),
        }
    }
}

impl Progress {
    pub fn log(&self, line: impl Into<String>) {
        let mut l = self.log.lock().unwrap_or_else(|p| p.into_inner());
        if l.len() < 20_000 {
            l.push(line.into());
        }
    }

    pub fn take_log(&self) -> Vec<String> {
        std::mem::take(&mut *self.log.lock().unwrap_or_else(|p| p.into_inner()))
    }

    pub fn set_phase(&self, phase: impl Into<String>) {
        *self.phase.lock().unwrap_or_else(|p| p.into_inner()) = phase.into();
        self.touch();
    }

    pub fn phase(&self) -> String {
        self.phase.lock().unwrap_or_else(|p| p.into_inner()).clone()
    }

    /// The folder being read right now.
    pub fn set_current(&self, path: &str) {
        *self.current.lock().unwrap_or_else(|p| p.into_inner()) = (path.to_string(), Instant::now());
    }

    pub fn current(&self) -> String {
        self.current.lock().unwrap_or_else(|p| p.into_inner()).0.clone()
    }

    /// Mark activity without changing the current folder.
    pub fn touch(&self) {
        self.current.lock().unwrap_or_else(|p| p.into_inner()).1 = Instant::now();
    }

    /// Time since the scan last made progress (a long stall usually means a
    /// macOS privacy prompt is waiting for an answer).
    pub fn stalled_for(&self) -> StdDuration {
        self.current.lock().unwrap_or_else(|p| p.into_inner()).1.elapsed()
    }
}

pub type LastUsedFn = dyn Fn(&Path) -> Option<DateTime<Utc>> + Sync;

pub fn mtime_of(md: &std::fs::Metadata) -> DateTime<Utc> {
    Utc.timestamp_opt(md.mtime(), md.mtime_nsec() as u32).single().unwrap_or_else(crate::model::zero_time)
}

pub fn scan(
    home: &Path,
    s: &Settings,
    last_used: Option<&LastUsedFn>,
    cancel: &AtomicBool,
) -> Result<ScanResult, ScanError> {
    scan_with_progress(home, s, last_used, cancel, &Progress::default())
}

fn short(path: &Path, home: &Path) -> String {
    match path.strip_prefix(home) {
        Ok(rel) if rel.as_os_str().is_empty() => "~".into(),
        Ok(rel) => format!("~/{}", rel.display()),
        Err(_) => path.display().to_string(),
    }
}

pub fn scan_with_progress(
    home: &Path,
    s: &Settings,
    last_used: Option<&LastUsedFn>,
    cancel: &AtomicBool,
    progress: &Progress,
) -> Result<ScanResult, ScanError> {
    s.validate().map_err(ScanError::Invalid)?;
    let roots = s.effective_roots(home);
    let mut result = ScanResult::default();
    let mut seen = HashSet::new();
    let mut volumes: HashMap<u64, Volume> = HashMap::new();
    let cutoff = Utc::now() - Duration::days(s.min_age_days);
    let home_clean = clean(home);
    let mut folders: HashMap<String, i64> = HashMap::new();
    let blocked = s.blocked_private(home);

    for root in roots {
        if cancel.load(Ordering::Relaxed) {
            return Err(ScanError::Cancelled);
        }
        let root = clean(&root);
        progress.set_phase(format!("Scanning {}", short(&root, home)));
        progress.log(format!("▶ Scanning folder {}", short(&root, home)));
        if blocked.iter().any(|b| within(&root, b)) {
            result.skipped += 1;
            progress.log(format!("  Skipped {} (no access: not allowed in Settings)", short(&root, home)));
            continue;
        }
        if (root != home_clean || !s.scan_home) && protected(&root, home, &s.exclusions) {
            result.skipped += 1;
            progress.log(format!("  Skipped {} (protected)", short(&root, home)));
            continue;
        }
        let Ok(info) = std::fs::symlink_metadata(&root) else {
            result.skipped += 1;
            progress.log(format!("  Skipped {} (not found)", short(&root, home)));
            continue;
        };
        if !info.is_dir() || info.file_type().is_symlink() {
            result.skipped += 1;
            progress.log(format!("  Skipped {} (not a folder)", short(&root, home)));
            continue;
        }
        let root_dev = info.dev();
        if let std::collections::hash_map::Entry::Vacant(e) = volumes.entry(root_dev) {
            if let Ok(v) = volume_stats(&root, s) {
                e.insert(v);
            }
        }
        let mut it = WalkDir::new(&root).follow_links(false).into_iter();
        while let Some(entry) = it.next() {
            if cancel.load(Ordering::Relaxed) {
                return Err(ScanError::Cancelled);
            }
            let entry = match entry {
                Ok(e) => e,
                Err(err) => {
                    result.skipped += 1;
                    progress.skipped.fetch_add(1, Ordering::Relaxed);
                    let p = err.path().map(|p| short(p, home)).unwrap_or_default();
                    let why = err.io_error().map(|e| e.to_string()).unwrap_or_else(|| err.to_string());
                    progress.log(format!("  ⚠ Cannot read {p}: {why}"));
                    continue;
                }
            };
            let path = entry.path();
            let ft = entry.file_type();
            let depth = entry.depth();
            if ft.is_dir() && blocked.iter().any(|b| b == path) {
                it.skip_current_dir();
                progress.log(format!("  Skipped {} (no access: not allowed in Settings)", short(path, home)));
                continue;
            }
            if path != root && protected(path, home, &s.exclusions) {
                if ft.is_dir() {
                    it.skip_current_dir();
                    if depth <= 2 {
                        progress.log(format!("  Skipped {} (protected)", short(path, home)));
                    }
                }
                continue;
            }
            if ft.is_symlink() {
                continue;
            }
            if ft.is_dir() && std::fs::symlink_metadata(path.join(".git")).is_ok() {
                it.skip_current_dir();
                if depth <= 2 {
                    progress.log(format!("  Skipped {} (git repository)", short(path, home)));
                }
                continue;
            }
            if ft.is_dir() {
                progress.dirs.fetch_add(1, Ordering::Relaxed);
                progress.set_current(&short(path, home));
                if (1..=2).contains(&depth) {
                    progress.log(format!("  {}", short(path, home)));
                }
            } else {
                progress.files.fetch_add(1, Ordering::Relaxed);
                progress.touch();
            }
            let Ok(md) = entry.metadata() else {
                result.skipped += 1;
                continue;
            };
            if md.dev() != root_dev {
                if ft.is_dir() {
                    it.skip_current_dir();
                }
                continue;
            }
            if md.is_file() {
                let key = match path.strip_prefix(&root).ok().and_then(|r| r.components().next()) {
                    Some(first) if path.strip_prefix(&root).map(|r| r.components().count() > 1).unwrap_or(false) => {
                        root.join(first).to_string_lossy().into_owned()
                    }
                    _ => root.to_string_lossy().into_owned(),
                };
                *folders.entry(key).or_default() += md.blocks() as i64 * 512;
            }
            let modified = mtime_of(&md);
            if !md.is_file() || (md.len() as i64) < s.min_size_bytes || modified > cutoff {
                continue;
            }
            let id = format!("{}:{}", md.dev(), md.ino());
            if !seen.insert(id.clone()) {
                continue;
            }
            progress.found.fetch_add(1, Ordering::Relaxed);
            progress.log(format!("  ★ Found {} · {}", crate::format::bytes(md.len() as i64), short(path, home)));
            result.candidates.push(Candidate {
                id,
                path: path.to_string_lossy().into_owned(),
                size: md.len() as i64,
                modified_at: modified,
                last_used_at: crate::model::zero_time(),
                last_use_known: false,
                device: md.dev(),
                inode: md.ino(),
                safety: Safety::Review,
                reason: "The file is large and old; last use is not yet known.".into(),
                ignored: false,
            });
        }
    }

    // Spotlight lookups are slow (a process each): run them on a few threads.
    let lookup: &LastUsedFn = last_used.unwrap_or(&spotlight_last_used);
    progress.lookups_total.store(result.candidates.len(), Ordering::Relaxed);
    if !result.candidates.is_empty() {
        progress.set_phase("Checking when files were last opened");
        progress.log(format!("▶ Checking last-used dates of {} files (Spotlight)", result.candidates.len()));
    }
    let used: Vec<Option<DateTime<Utc>>> = {
        let paths: Vec<&str> = result.candidates.iter().map(|c| c.path.as_str()).collect();
        let workers = 8.min(paths.len().max(1));
        let chunk = paths.len().div_ceil(workers).max(1);
        std::thread::scope(|scope| {
            let handles: Vec<_> = paths
                .chunks(chunk)
                .map(|part| {
                    scope.spawn(move || {
                        part.iter()
                            .map(|p| {
                                if cancel.load(Ordering::Relaxed) {
                                    return None;
                                }
                                let r = lookup(Path::new(p));
                                progress.lookups_done.fetch_add(1, Ordering::Relaxed);
                                progress.touch();
                                r
                            })
                            .collect::<Vec<_>>()
                    })
                })
                .collect();
            handles.into_iter().flat_map(|h| h.join().unwrap_or_default()).collect()
        })
    };
    if cancel.load(Ordering::Relaxed) {
        return Err(ScanError::Cancelled);
    }
    for (c, u) in result.candidates.iter_mut().zip(used) {
        if let Some(t) = u {
            c.last_used_at = t;
            c.last_use_known = true;
            c.reason = if t > cutoff {
                "The file was opened recently; review before cleaning.".into()
            } else {
                "Old large file; awaiting AI review.".into()
            };
        }
    }
    result.candidates.sort_by_key(|c| std::cmp::Reverse(c.size));
    result.folder_sizes = folders.into_iter().collect();
    result.folder_sizes.sort_by_key(|(_, size)| std::cmp::Reverse(*size));
    result.volumes = volumes.into_values().collect();
    result.volumes.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(result)
}

fn cstr(buf: &[libc::c_char]) -> String {
    unsafe { CStr::from_ptr(buf.as_ptr()) }.to_string_lossy().into_owned()
}

/// Free/total space and identity of the volume holding `path`.
pub fn volume_stats(path: &Path, s: &Settings) -> std::io::Result<Volume> {
    let md = std::fs::symlink_metadata(path)?;
    let c = std::ffi::CString::new(path.as_os_str().as_encoded_bytes())
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidInput, e))?;
    let mut st: libc::statfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statfs(c.as_ptr(), &mut st) } != 0 {
        return Err(std::io::Error::last_os_error());
    }
    let free = st.f_bavail as i64 * st.f_bsize as i64;
    let total = st.f_blocks as i64 * st.f_bsize as i64;
    let mount = cstr(&st.f_mntonname);
    let from = cstr(&st.f_mntfromname);
    Ok(Volume {
        name: volume_name(&mount),
        path: mount,
        device: md.dev() as u64,
        free_bytes: free,
        total_bytes: total,
        fs_type: cstr(&st.f_fstypename).to_uppercase(),
        bsd_name: from.trim_start_matches("/dev/").to_string(),
        level: level(free, s.red_gb, s.yellow_gb),
    })
}

pub fn volume_name(mount: &str) -> String {
    if mount == "/" || mount == "/System/Volumes/Data" {
        return "Macintosh HD".into();
    }
    Path::new(mount).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| mount.to_string())
}

/// `kMDItemLastUsedDate` via `mdls`, 3 s timeout.
pub fn spotlight_last_used(path: &Path) -> Option<DateTime<Utc>> {
    let mut cmd = Command::new("/usr/bin/mdls");
    cmd.args(["-raw", "-name", "kMDItemLastUsedDate"]).arg(path);
    let out = crate::proc::run(cmd, None, std::time::Duration::from_secs(3)).ok()?;
    if !out.status.success() {
        return None;
    }
    parse_mdls_date(String::from_utf8_lossy(&out.stdout).trim())
}

pub fn parse_mdls_date(value: &str) -> Option<DateTime<Utc>> {
    if value.is_empty() || value == "(null)" {
        return None;
    }
    DateTime::parse_from_str(value, "%Y-%m-%d %H:%M:%S %z")
        .or_else(|_| DateTime::parse_from_rfc3339(value))
        .ok()
        .map(|t| t.with_timezone(&Utc))
}
