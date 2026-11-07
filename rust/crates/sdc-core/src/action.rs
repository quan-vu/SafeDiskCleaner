//! Re-validation before any destructive action. Mirrors Go `action.go`.

use crate::model::{protected, within, Candidate, Settings};
use crate::scan::mtime_of;
use std::os::unix::fs::MetadataExt;
use std::path::Path;

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ActionError {
    #[error("file changed since scan; scan again")]
    Stale,
    #[error("protected path")]
    Protected,
    #[error("outside selected scan folders")]
    OutsideRoots,
    #[error("{0}")]
    Io(String),
}

pub fn validate_for_action(c: &Candidate, home: &Path, s: &Settings) -> Result<(), ActionError> {
    let path = Path::new(&c.path);
    if protected(path, home, &s.exclusions) {
        return Err(ActionError::Protected);
    }
    let roots = s.effective_roots(home);
    let Some(root) = roots.iter().find(|r| within(path, r)) else {
        return Err(ActionError::OutsideRoots);
    };
    // Reject a parent replaced by a symlink (or removed) after the scan.
    let mut parent = path.parent();
    while let Some(dir) = parent {
        if !within(dir, root) {
            break;
        }
        match std::fs::symlink_metadata(dir) {
            Ok(md) if md.is_dir() && !md.file_type().is_symlink() => {}
            _ => return Err(ActionError::Stale),
        }
        if dir == root.as_path() {
            break;
        }
        parent = dir.parent();
    }
    let md = std::fs::symlink_metadata(path).map_err(|_| ActionError::Stale)?;
    if !md.is_file()
        || md.file_type().is_symlink()
        || md.dev() as u64 != c.device
        || md.ino() != c.inode
        || md.len() as i64 != c.size
        || mtime_of(&md) != c.modified_at
    {
        return Err(ActionError::Stale);
    }
    Ok(())
}

pub fn delete_permanently(c: &Candidate, home: &Path, s: &Settings) -> Result<(), ActionError> {
    validate_for_action(c, home, s)?;
    std::fs::remove_file(&c.path).map_err(|e| ActionError::Io(e.to_string()))
}
