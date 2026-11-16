//! Data model and local safety rules. Behavior mirrors the Go `internal/cleaner/model.go`.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Deserializer, Serialize};
use std::path::{Component, Path, PathBuf};

pub const GB: i64 = 1_000_000_000;
pub const MB: i64 = 1_000_000;

/// Go zero time (`0001-01-01T00:00:00Z`), used for "never".
pub fn zero_time() -> DateTime<Utc> {
    DateTime::parse_from_rfc3339("0001-01-01T00:00:00Z").unwrap().with_timezone(&Utc)
}

pub fn is_zero_time(t: &DateTime<Utc>) -> bool {
    *t <= zero_time()
}

fn null_vec<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<String>, D::Error> {
    Ok(Option::<Vec<String>>::deserialize(d)?.unwrap_or_default())
}

fn default_time() -> DateTime<Utc> {
    zero_time()
}

pub const DEFAULT_SCAN_INTERVAL_MINUTES: i64 = 10;

fn default_true() -> bool {
    true
}

fn default_menu_items() -> Vec<String> {
    vec!["disk".into()]
}

/// Home folders macOS guards with a privacy prompt. The app only reads the ones
/// listed in `Settings::private_folders`.
pub const PRIVATE_FOLDERS: [&str; 3] = ["Desktop", "Documents", "Downloads"];

fn default_private_folders() -> Vec<String> {
    vec!["Documents".into(), "Downloads".into()]
}

fn default_menu_style() -> String {
    "compact".into()
}

fn default_interval() -> i64 {
    DEFAULT_SCAN_INTERVAL_MINUTES
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    #[serde(default)]
    pub setup_done: bool,
    #[serde(default)]
    pub auto_scan: bool,
    #[serde(default)]
    pub open_at_login: bool,
    #[serde(default)]
    pub scan_home: bool,
    /// Opt-in: sends paths, sizes and dates to the local `claude` CLI. Off unless the user turns it on.
    #[serde(default)]
    pub ai_review: bool,
    /// Minutes between automatic scans.
    #[serde(default = "default_interval")]
    pub scan_interval_minutes: i64,
    /// Overview cards in display order with their size.
    #[serde(default = "crate::overview::default_cards")]
    pub overview_cards: Vec<crate::overview::CardPref>,
    /// Values shown next to the menu bar dot: "disk", "temp", "cpu", "ram".
    #[serde(default = "default_menu_items")]
    pub menu_bar_items: Vec<String>,
    /// "compact" (values), "labels" or "icons".
    #[serde(default = "default_menu_style")]
    pub menu_bar_style: String,
    #[serde(default = "default_true")]
    pub menu_bar_dot: bool,
    /// Storage page also measures iCloud Drive and cloud folders (macOS asks for access). Off by default.
    #[serde(default)]
    pub include_icloud: bool,
    /// Privacy-guarded home folders the app may read (others are never opened, so macOS never asks).
    #[serde(default = "default_private_folders")]
    pub private_folders: Vec<String>,
    #[serde(default, deserialize_with = "null_vec")]
    pub roots: Vec<String>,
    #[serde(default, deserialize_with = "null_vec")]
    pub exclusions: Vec<String>,
    pub min_size_bytes: i64,
    pub min_age_days: i64,
    #[serde(rename = "redGB")]
    pub red_gb: i64,
    #[serde(rename = "yellowGB")]
    pub yellow_gb: i64,
    #[serde(default = "default_time")]
    pub last_scan: DateTime<Utc>,
    #[serde(default)]
    pub last_alert_level: String,
    #[serde(default, rename = "lastAlertIds", deserialize_with = "null_vec")]
    pub last_alert_ids: Vec<String>,
}

impl Settings {
    pub fn default_for(home: &Path) -> Self {
        Settings {
            setup_done: false,
            auto_scan: true,
            open_at_login: true,
            scan_home: false,
            ai_review: false,
            scan_interval_minutes: DEFAULT_SCAN_INTERVAL_MINUTES,
            overview_cards: crate::overview::default_cards(),
            menu_bar_items: default_menu_items(),
            menu_bar_style: default_menu_style(),
            menu_bar_dot: true,
            include_icloud: false,
            private_folders: default_private_folders(),
            roots: vec![
                home.join("Downloads").to_string_lossy().into_owned(),
                home.join("Documents").to_string_lossy().into_owned(),
            ],
            exclusions: vec![],
            min_size_bytes: 500 * MB,
            min_age_days: 90,
            red_gb: 10,
            yellow_gb: 30,
            last_scan: zero_time(),
            last_alert_level: String::new(),
            last_alert_ids: vec![],
        }
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.scan_interval_minutes < 1 {
            return Err("scan interval must be at least one minute".into());
        }
        if self.min_size_bytes < 1 || self.min_age_days < 1 || self.red_gb < 1 || self.yellow_gb <= self.red_gb {
            return Err("invalid size, age, or disk thresholds".into());
        }
        if !self.scan_home && self.roots.is_empty() {
            return Err("select at least one scan folder".into());
        }
        for p in self.roots.iter().chain(self.exclusions.iter()) {
            let path = Path::new(p);
            if !path.is_absolute() || clean(path) != path {
                return Err("scan paths must be absolute and clean".into());
            }
        }
        Ok(())
    }

    /// Privacy-guarded home folders the app must not open.
    pub fn blocked_private(&self, home: &Path) -> Vec<PathBuf> {
        PRIVATE_FOLDERS.iter().filter(|f| !self.private_folders.iter().any(|p| p == *f)).map(|f| home.join(f)).collect()
    }

    /// The privacy-guarded home folder `path` lies in, if any ("Desktop"…).
    pub fn private_folder_of(path: &Path, home: &Path) -> Option<&'static str> {
        PRIVATE_FOLDERS.iter().copied().find(|f| within(path, &home.join(f)))
    }

    /// Roots actually scanned: home when `scan_home`, else the selected folders.
    pub fn effective_roots(&self, home: &Path) -> Vec<PathBuf> {
        if self.scan_home {
            vec![home.to_path_buf()]
        } else {
            self.roots.iter().map(PathBuf::from).collect()
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Safety {
    Protected,
    Review,
    Safe,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Candidate {
    pub id: String,
    pub path: String,
    pub size: i64,
    pub modified_at: DateTime<Utc>,
    #[serde(default = "default_time")]
    pub last_used_at: DateTime<Utc>,
    #[serde(default)]
    pub last_use_known: bool,
    pub device: u64,
    pub inode: u64,
    pub safety: Safety,
    #[serde(default)]
    pub reason: String,
    #[serde(default)]
    pub ignored: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Level {
    #[default]
    Green,
    Yellow,
    Red,
}

impl Level {
    pub fn as_str(self) -> &'static str {
        match self {
            Level::Green => "green",
            Level::Yellow => "yellow",
            Level::Red => "red",
        }
    }
    pub fn parse(s: &str) -> Level {
        match s {
            "red" => Level::Red,
            "yellow" => Level::Yellow,
            _ => Level::Green,
        }
    }
}

pub fn level(free: i64, red_gb: i64, yellow_gb: i64) -> Level {
    if free <= red_gb * GB {
        Level::Red
    } else if free <= yellow_gb * GB {
        Level::Yellow
    } else {
        Level::Green
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Volume {
    pub path: String,
    pub name: String,
    pub device: u64,
    pub free_bytes: i64,
    pub total_bytes: i64,
    pub fs_type: String,
    pub bsd_name: String,
    pub level: Level,
}

/// Lexical clean like Go's filepath.Clean (no filesystem access).
pub fn clean(p: &Path) -> PathBuf {
    let mut out: Vec<Component> = Vec::new();
    for c in p.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => match out.last() {
                Some(Component::Normal(_)) => {
                    out.pop();
                }
                Some(Component::RootDir) => {}
                _ => out.push(c),
            },
            other => out.push(other),
        }
    }
    if out.is_empty() {
        return PathBuf::from(".");
    }
    out.iter().collect()
}

/// True when `path` equals `dir` or lies beneath it (lexically).
pub fn within(path: &Path, dir: &Path) -> bool {
    clean(path).starts_with(clean(dir))
}

const SYSTEM_DIRS: &[&str] = &[
    "/System", "/Library", "/Applications", "/private", "/usr", "/bin", "/sbin", "/dev", "/etc", "/opt", "/var",
];

const HOME_PROTECTED_PARTS: &[&str] = &[
    "library", ".trash", ".git", ".ssh", ".aws", ".config", ".claude", ".local", ".npm", ".cache", "node_modules",
    "photos library.photoslibrary", "music library.musiclibrary", "mobile documents", "icloud drive", "onedrive",
    "dropbox", "google drive",
];

const PROTECTED_SUFFIXES: &[&str] = &[".app", ".photoslibrary", ".musiclibrary", ".bundle", ".framework"];

pub fn protected(path: &Path, home: &Path, exclusions: &[String]) -> bool {
    let p = clean(path);
    let h = clean(home);
    if !p.is_absolute() || p == Path::new("/") || p == h {
        return true;
    }
    let in_home = within(&p, &h);
    if !in_home && SYSTEM_DIRS.iter().any(|d| within(&p, Path::new(d))) {
        return true;
    }
    if exclusions.iter().any(|x| within(&p, Path::new(x))) {
        return true;
    }
    if in_home {
        if let Ok(rel) = p.strip_prefix(&h) {
            for part in rel.components() {
                let lower = part.as_os_str().to_string_lossy().to_lowercase();
                if HOME_PROTECTED_PARTS.contains(&lower.as_str()) {
                    return true;
                }
            }
        }
    }
    for part in p.components() {
        let Component::Normal(os) = part else { continue };
        let s = os.to_string_lossy();
        let lower = s.to_lowercase();
        if s.starts_with('.') {
            return true;
        }
        if lower.starts_with("onedrive")
            || lower.starts_with("dropbox")
            || lower.starts_with("google drive")
            || lower.starts_with("icloud")
            || lower == "cloudstorage"
        {
            return true;
        }
        if PROTECTED_SUFFIXES.iter().any(|x| lower.ends_with(x)) {
            return true;
        }
    }
    false
}

pub fn worst_level(volumes: &[Volume]) -> Level {
    volumes.iter().map(|v| v.level).max().unwrap_or(Level::Green)
}

/// IDs of non-ignored candidates, sorted (used for change detection).
pub fn candidate_ids(items: &[Candidate]) -> Vec<String> {
    let mut ids: Vec<String> = items.iter().filter(|c| !c.ignored).map(|c| c.id.clone()).collect();
    ids.sort();
    ids
}

/// Notify when the disk level changed or a new non-ignored suggestion appeared.
pub fn should_notify(before: &[Candidate], after: &[Candidate], before_level: Level, after_level: Level) -> bool {
    if before_level != after_level {
        return true;
    }
    after.iter().any(|c| !c.ignored && !before.iter().any(|b| b.id == c.id))
}

/// True when the file a candidate refers to is still on disk (same device and inode).
pub fn still_exists(c: &Candidate) -> bool {
    use std::os::unix::fs::MetadataExt;
    match std::fs::symlink_metadata(&c.path) {
        Ok(md) => md.is_file() && md.dev() == c.device && md.ino() == c.inode,
        Err(_) => false,
    }
}

/// Copy the previous AI verdict onto candidates whose file is unchanged since
/// the last scan; returns the indices of candidates that still need a review.
/// A previous entry whose reason equals the fresh scan reason was never
/// reviewed (or its review failed), so it is reviewed again.
pub fn carry_over(before: &[Candidate], after: &mut [Candidate]) -> Vec<usize> {
    let prev: std::collections::HashMap<&str, &Candidate> = before.iter().map(|c| (c.id.as_str(), c)).collect();
    let mut todo = vec![];
    for (i, c) in after.iter_mut().enumerate() {
        match prev.get(c.id.as_str()) {
            Some(b)
                if b.path == c.path
                    && b.size == c.size
                    && b.modified_at == c.modified_at
                    && b.last_used_at == c.last_used_at
                    && b.last_use_known == c.last_use_known
                    && b.reason != c.reason =>
            {
                c.safety = b.safety;
                c.reason = b.reason.clone();
            }
            _ => todo.push(i),
        }
    }
    todo
}
