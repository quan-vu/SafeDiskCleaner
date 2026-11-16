//! Daily disk snapshots: one per day (the latest scan of the day wins), so the
//! app can show how volumes and scanned folders grew or shrank.

use crate::model::{Candidate, Volume};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VolumeSnap {
    pub name: String,
    pub path: String,
    pub free_bytes: i64,
    pub total_bytes: i64,
}

impl VolumeSnap {
    pub fn used(&self) -> i64 {
        self.total_bytes - self.free_bytes
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileSnap {
    pub id: String,
    pub path: String,
    pub size: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    /// Local date, `YYYY-MM-DD`.
    pub day: String,
    /// RFC 3339 time of the scan that produced it.
    pub at: String,
    pub volumes: Vec<VolumeSnap>,
    /// (folder path, on-disk bytes), largest first.
    pub folders: Vec<(String, i64)>,
    /// Large files found by the scan (largest 200).
    pub files: Vec<FileSnap>,
}

impl Snapshot {
    pub fn new(day: &str, at: &str, volumes: &[Volume], folders: &[(String, i64)], files: &[Candidate]) -> Snapshot {
        let mut files: Vec<FileSnap> =
            files.iter().map(|c| FileSnap { id: c.id.clone(), path: c.path.clone(), size: c.size }).collect();
        files.sort_by_key(|f| std::cmp::Reverse(f.size));
        files.truncate(200);
        Snapshot {
            day: day.into(),
            at: at.into(),
            volumes: volumes
                .iter()
                .map(|v| VolumeSnap {
                    name: v.name.clone(),
                    path: v.path.clone(),
                    free_bytes: v.free_bytes,
                    total_bytes: v.total_bytes,
                })
                .collect(),
            folders: folders.to_vec(),
            files,
        }
    }

    pub fn used(&self) -> i64 {
        self.volumes.iter().map(VolumeSnap::used).sum()
    }
}

/// What changed between two snapshots.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Change {
    /// Change in used space over all volumes (positive = disk filled up).
    pub used_delta: i64,
    /// (path, before, after) for folders whose size changed, biggest change first.
    pub folders: Vec<(String, i64, i64)>,
    pub new_files: Vec<FileSnap>,
    pub gone_files: Vec<FileSnap>,
}

pub fn compare(before: &Snapshot, after: &Snapshot) -> Change {
    let old: HashMap<&str, i64> = before.folders.iter().map(|(p, s)| (p.as_str(), *s)).collect();
    let new: HashMap<&str, i64> = after.folders.iter().map(|(p, s)| (p.as_str(), *s)).collect();
    let mut paths: Vec<&str> = old.keys().chain(new.keys()).copied().collect::<HashSet<_>>().into_iter().collect();
    paths.sort();
    let mut folders: Vec<(String, i64, i64)> = paths
        .into_iter()
        .map(|p| (p.to_string(), old.get(p).copied().unwrap_or(0), new.get(p).copied().unwrap_or(0)))
        .filter(|(_, a, b)| a != b)
        .collect();
    folders.sort_by_key(|(_, a, b)| std::cmp::Reverse((b - a).abs()));
    let old_ids: HashSet<&str> = before.files.iter().map(|f| f.id.as_str()).collect();
    let new_ids: HashSet<&str> = after.files.iter().map(|f| f.id.as_str()).collect();
    Change {
        used_delta: after.used() - before.used(),
        folders,
        new_files: after.files.iter().filter(|f| !old_ids.contains(f.id.as_str())).cloned().collect(),
        gone_files: before.files.iter().filter(|f| !new_ids.contains(f.id.as_str())).cloned().collect(),
    }
}
