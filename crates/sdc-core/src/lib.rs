//! Safety Disk Cleaner core: scanning, safety rules, AI review, persistence and
//! read-only system statistics. No UI code lives here.

pub mod action;
pub mod category;
pub mod format;
pub mod model;
pub mod overview;
pub mod proc;
pub mod review;
pub mod scan;
pub mod snapshot;
pub mod store;
pub mod usage;
pub mod sysstats;

#[cfg(test)]
mod tests;
