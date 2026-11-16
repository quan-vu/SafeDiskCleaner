//! Space used per folder: the home folder split by its top-level entries, and
//! the disk split into Home, Applications, Library, System & other, and Free.

use serde::{Deserialize, Serialize};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Part {
    pub label: String,
    /// Folder to reveal in Finder; empty for computed parts (Free, Other…).
    pub path: String,
    pub bytes: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Usage {
    /// RFC 3339 time of the measurement.
    pub at: String,
    /// Largest first.
    pub parts: Vec<Part>,
}

impl Usage {
    pub fn total(&self) -> i64 {
        self.parts.iter().map(|p| p.bytes).sum()
    }
}

/// Allocated bytes under `dir`, staying on its volume, skipping symlinks,
/// unreadable entries and any directory in `skip`.
pub fn dir_size_skipping(dir: &Path, skip: &[PathBuf], cancel: &dyn Fn() -> bool) -> i64 {
    let Ok(root) = std::fs::symlink_metadata(dir) else { return 0 };
    if root.file_type().is_symlink() {
        return 0;
    }
    if root.is_file() {
        return root.blocks() as i64 * 512;
    }
    let dev = root.dev();
    let mut total = 0i64;
    let mut it = walkdir::WalkDir::new(dir).follow_links(false).into_iter();
    while let Some(entry) = it.next() {
        if cancel() {
            break;
        }
        let Ok(entry) = entry else { continue };
        if entry.file_type().is_dir() && skip.iter().any(|s| s == entry.path()) {
            it.skip_current_dir();
            continue;
        }
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

/// iCloud Drive and other cloud-synced folders. Reading them makes macOS ask for
/// access (and can start downloads), so they are skipped unless asked for.
pub fn cloud_paths(home: &Path) -> Vec<PathBuf> {
    vec![
        home.join("Library/Mobile Documents"),
        home.join("Library/CloudStorage"),
        home.join("Library/Application Support/CloudDocs"),
    ]
}

/// A home-level name that is an iCloud folder ("iCloud Drive", "iCloud Drive (Archive)"…).
pub fn is_icloud_name(name: &str) -> bool {
    name.to_lowercase().starts_with("icloud")
}

/// Folders the measurement never walks: other apps' sandboxes in ~/Library
/// (macOS would ask "allow access to data from other apps"), plus the cloud
/// folders unless `include_cloud`.
pub fn home_skips(home: &Path, include_cloud: bool) -> Vec<PathBuf> {
    let mut v = vec![home.join("Library/Containers"), home.join("Library/Group Containers")];
    if !include_cloud {
        v.extend(cloud_paths(home));
    }
    v
}

/// Top-level entries of the home folder with their sizes, largest first.
/// Loose files directly in home are summed into one "Files in Home" part.
/// `blocked`: folders never opened (privacy-guarded folders the user hasn't allowed).
pub fn home_usage(
    home: &Path,
    include_cloud: bool,
    blocked: &[PathBuf],
    at: &str,
    progress: &dyn Fn(&str),
    cancel: &dyn Fn() -> bool,
) -> Usage {
    let mut skip = home_skips(home, include_cloud);
    skip.extend(blocked.iter().cloned());
    let mut parts = vec![];
    let mut loose = 0i64;
    let mut entries: Vec<_> = std::fs::read_dir(home).map(|r| r.flatten().collect()).unwrap_or_default();
    entries.sort_by_key(|e| e.file_name());
    for e in entries {
        if cancel() {
            break;
        }
        let path = e.path();
        let Ok(md) = std::fs::symlink_metadata(&path) else { continue };
        if md.file_type().is_symlink() {
            continue;
        }
        if md.is_file() {
            loose += md.blocks() as i64 * 512;
            continue;
        }
        if !md.is_dir() {
            continue;
        }
        let name = e.file_name().to_string_lossy().into_owned();
        if (!include_cloud && is_icloud_name(&name)) || skip.contains(&path) {
            continue;
        }
        progress(&format!("~/{name}"));
        let bytes = dir_size_skipping(&path, &skip, cancel);
        if bytes > 0 {
            parts.push(Part { label: name, path: path.to_string_lossy().into_owned(), bytes });
        }
    }
    if loose > 0 {
        parts.push(Part { label: "Files in Home".into(), path: home.to_string_lossy().into_owned(), bytes: loose });
    }
    parts.sort_by_key(|p| std::cmp::Reverse(p.bytes));
    Usage { at: at.into(), parts }
}

/// The disk split into Home, Applications, Library, System & other, and Free.
/// `home_total` comes from `home_usage`; the volume's total and free bytes from statfs.
pub fn disk_usage(
    home: &Path,
    home_total: i64,
    volume_total: i64,
    volume_free: i64,
    at: &str,
    progress: &dyn Fn(&str),
    cancel: &dyn Fn() -> bool,
) -> Usage {
    progress("/Applications");
    let apps = dir_size_skipping(Path::new("/Applications"), &[], cancel);
    progress("/Library");
    let library = dir_size_skipping(Path::new("/Library"), &[], cancel);
    disk_parts(home, home_total, apps, library, volume_total, volume_free, at)
}

/// Assemble the disk split from measured sizes; "System & other" is what is left of used space.
pub fn disk_parts(home: &Path, home_total: i64, apps: i64, library: i64, volume_total: i64, volume_free: i64, at: &str) -> Usage {
    let used = (volume_total - volume_free).max(0);
    let other = (used - home_total - apps - library).max(0);
    let mut parts = vec![
        Part { label: "Home".into(), path: home.to_string_lossy().into_owned(), bytes: home_total },
        Part { label: "Applications".into(), path: "/Applications".into(), bytes: apps },
        Part { label: "Library".into(), path: "/Library".into(), bytes: library },
        Part { label: "System & other".into(), path: String::new(), bytes: other },
    ];
    parts.retain(|p| p.bytes > 0);
    parts.sort_by_key(|p| std::cmp::Reverse(p.bytes));
    parts.push(Part { label: "Free".into(), path: String::new(), bytes: volume_free.max(0) });
    Usage { at: at.into(), parts }
}

/// Keep the `max - 1` largest parts and fold the rest into "Other" (so at most `max`).
/// Parts listed in `keep_last` (e.g. "Free") stay last and are never folded.
pub fn fold(parts: &[Part], max: usize, keep_last: &[&str]) -> Vec<Part> {
    let (tail, body): (Vec<Part>, Vec<Part>) = parts.iter().cloned().partition(|p| keep_last.contains(&p.label.as_str()));
    let room = max.saturating_sub(tail.len()).max(1);
    let mut out: Vec<Part> = if body.len() > room {
        let mut v: Vec<Part> = body[..room - 1].to_vec();
        let rest: i64 = body[room - 1..].iter().map(|p| p.bytes).sum();
        v.push(Part { label: "Other".into(), path: String::new(), bytes: rest });
        v
    } else {
        body
    };
    out.extend(tail);
    out
}
