//! Filesystem scan. Same skip rules as the Go `scan.go`.

use crate::model::{clean, level, protected, Candidate, Safety, Settings, Volume};
use chrono::{DateTime, Duration, TimeZone, Utc};
use std::collections::{HashMap, HashSet};
use std::ffi::CStr;
use std::os::unix::fs::MetadataExt;
use std::path::Path;
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use walkdir::WalkDir;

#[derive(Debug, Default)]
pub struct ScanResult {
    pub candidates: Vec<Candidate>,
    pub volumes: Vec<Volume>,
    pub skipped: usize,
}

#[derive(Debug, thiserror::Error)]
pub enum ScanError {
    #[error("{0}")]
    Invalid(String),
    #[error("scan cancelled")]
    Cancelled,
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
    s.validate().map_err(ScanError::Invalid)?;
    let roots = s.effective_roots(home);
    let mut result = ScanResult::default();
    let mut seen = HashSet::new();
    let mut volumes: HashMap<u64, Volume> = HashMap::new();
    let cutoff = Utc::now() - Duration::days(s.min_age_days);
    let home_clean = clean(home);

    for root in roots {
        if cancel.load(Ordering::Relaxed) {
            return Err(ScanError::Cancelled);
        }
        let root = clean(&root);
        if (root != home_clean || !s.scan_home) && protected(&root, home, &s.exclusions) {
            result.skipped += 1;
            continue;
        }
        let Ok(info) = std::fs::symlink_metadata(&root) else {
            result.skipped += 1;
            continue;
        };
        if !info.is_dir() || info.file_type().is_symlink() {
            result.skipped += 1;
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
                Err(_) => {
                    result.skipped += 1;
                    continue;
                }
            };
            let path = entry.path();
            let ft = entry.file_type();
            if path != root && protected(path, home, &s.exclusions) {
                if ft.is_dir() {
                    it.skip_current_dir();
                }
                continue;
            }
            if ft.is_symlink() {
                continue;
            }
            if ft.is_dir() && std::fs::symlink_metadata(path.join(".git")).is_ok() {
                it.skip_current_dir();
                continue;
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
            let modified = mtime_of(&md);
            if !md.is_file() || (md.len() as i64) < s.min_size_bytes || modified > cutoff {
                continue;
            }
            let id = format!("{}:{}", md.dev(), md.ino());
            if !seen.insert(id.clone()) {
                continue;
            }
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
                            .map(|p| if cancel.load(Ordering::Relaxed) { None } else { lookup(Path::new(p)) })
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
