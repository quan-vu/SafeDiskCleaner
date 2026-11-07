use crate::action::{validate_for_action, ActionError};
use crate::model::*;
use crate::review::review;
use crate::scan::{parse_mdls_date, scan};
use crate::store::Store;
use crate::sysstats::{app_group, parse_hdiutil, RateHistory};
use chrono::{Duration, Utc};
use std::path::Path;
use std::sync::atomic::AtomicBool;
use std::time::Instant;

#[test]
fn level_and_rules() {
    assert_eq!(level(10 * GB, 10, 30), Level::Red);
    assert_eq!(level(30 * GB, 10, 30), Level::Yellow);
    assert_eq!(level(31 * GB, 10, 30), Level::Green);
    let home = Path::new("/Users/test");
    let excl = vec!["/Users/test/Downloads/keep".to_string()];
    for p in [
        "/System/file",
        "/Users/test/Library/cache",
        "/Users/test/Pictures/Photos Library.photoslibrary/a",
        "/Users/test/Downloads/.git/config",
        "/Users/test/.codex/sessions/log",
        "/Users/test/Downloads/keep/a",
        "/Users/test/Applications/Foo.app/Contents/x",
        "/Users/test/Dropbox (Personal)/a",
        "/Users/test",
        "/",
    ] {
        assert!(protected(Path::new(p), home, &excl), "not protected: {p}");
    }
    assert!(!protected(Path::new("/Users/test/Downloads/old.dmg"), home, &[]));
}

#[test]
fn clean_and_within() {
    assert_eq!(clean(Path::new("/a/./b/../c/")), Path::new("/a/c"));
    assert!(within(Path::new("/a/b/c"), Path::new("/a/b")));
    assert!(within(Path::new("/a/b"), Path::new("/a/b")));
    assert!(!within(Path::new("/a/bc"), Path::new("/a/b")));
}

fn set_age(path: &Path, days: i64) {
    let t = std::time::SystemTime::now() - std::time::Duration::from_secs(days as u64 * 86400);
    filetime::set_file_mtime(path, filetime::FileTime::from_system_time(t)).unwrap();
}

#[test]
fn scan_and_stale_file() {
    let tmp = tempfile::Builder::new().prefix("sdc").tempdir().unwrap();
    let home = std::fs::canonicalize(tmp.path()).unwrap();
    let root = home.join("Downloads");
    std::fs::create_dir(&root).unwrap();
    let old = root.join("old.dmg");
    let newer = root.join("new.dmg");
    for p in [&old, &newer] {
        std::fs::write(p, b"123456").unwrap();
    }
    set_age(&old, 120);
    std::os::unix::fs::symlink(&old, root.join("link.dmg")).unwrap();
    // A git checkout is skipped entirely.
    let repo = root.join("repo");
    std::fs::create_dir_all(repo.join(".git")).unwrap();
    std::fs::write(repo.join("big.bin"), b"1234567").unwrap();
    set_age(&repo.join("big.bin"), 200);

    let mut s = Settings::default_for(&home);
    s.min_size_bytes = 4;
    let past = Utc::now() - Duration::days(120);
    let cancel = AtomicBool::new(false);
    let lookup = move |_: &Path| Some(past);
    let result = scan(&home, &s, Some(&lookup), &cancel).unwrap();
    assert_eq!(result.candidates.len(), 1, "{:?}", result.candidates);
    let c = &result.candidates[0];
    assert_eq!(c.path, old.to_string_lossy());
    assert!(c.last_use_known);
    assert_eq!(result.volumes.len(), 1);
    validate_for_action(c, &home, &s).unwrap();

    std::fs::write(&old, b"changed").unwrap();
    assert_eq!(validate_for_action(c, &home, &s), Err(ActionError::Stale));

    // Home mode still scans children while protected areas stay out.
    s.scan_home = true;
    let none = |_: &Path| None;
    scan(&home, &s, Some(&none), &cancel).unwrap();
}

#[test]
fn symlinked_parent_is_rejected() {
    let tmp = tempfile::Builder::new().prefix("sdc").tempdir().unwrap();
    let home = std::fs::canonicalize(tmp.path()).unwrap();
    let root = home.join("Downloads");
    let sub = root.join("sub");
    std::fs::create_dir_all(&sub).unwrap();
    let f = sub.join("a.iso");
    std::fs::write(&f, b"123456").unwrap();
    set_age(&f, 200);
    let mut s = Settings::default_for(&home);
    s.min_size_bytes = 4;
    let none = |_: &Path| None;
    let r = scan(&home, &s, Some(&none), &AtomicBool::new(false)).unwrap();
    let c = r.candidates[0].clone();
    // Replace the parent directory by a symlink to a look-alike.
    let elsewhere = home.join("elsewhere");
    std::fs::rename(&sub, &elsewhere).unwrap();
    std::os::unix::fs::symlink(&elsewhere, &sub).unwrap();
    assert_eq!(validate_for_action(&c, &home, &s), Err(ActionError::Stale));
}

#[test]
fn cancelled_scan_stops() {
    let tmp = tempfile::Builder::new().prefix("sdc").tempdir().unwrap();
    let home = std::fs::canonicalize(tmp.path()).unwrap();
    std::fs::create_dir(home.join("Downloads")).unwrap();
    let s = Settings::default_for(&home);
    let r = scan(&home, &s, None, &AtomicBool::new(true));
    assert!(r.is_err());
}

#[test]
fn claude_cannot_promote_unknown_use_or_protected() {
    let home = Path::new("/Users/test");
    let s = Settings::default_for(home);
    let past = Utc::now() - Duration::days(120);
    let mk = |id: &str, path: &str, known: bool, safety: Safety| Candidate {
        id: id.into(),
        path: path.into(),
        size: 1,
        modified_at: past,
        last_used_at: if known { past } else { zero_time() },
        last_use_known: known,
        device: 1,
        inode: 1,
        safety,
        reason: String::new(),
        ignored: false,
    };
    let items = vec![
        mk("old", "/Users/test/Downloads/old.dmg", true, Safety::Review),
        mk("unknown", "/Users/test/Downloads/unknown.dmg", false, Safety::Review),
        mk("protected", "/Users/test/Library/private.db", true, Safety::Protected),
    ];
    let structured = serde_json::json!({"items": [
        {"id": "old", "verdict": "safe", "reason": "disposable"},
        {"id": "unknown", "verdict": "safe", "reason": "disposable"},
        {"id": "protected", "verdict": "safe", "reason": "disposable"},
        {"id": "old", "verdict": "review", "reason": "duplicate id ignored"},
    ]});
    let envelope = serde_json::to_vec(&serde_json::json!({ "structured_output": structured })).unwrap();
    let ok = move |_: &str, args: &[String], _: &[u8]| {
        assert!(args.iter().any(|a| a == "--safe-mode"));
        Ok(envelope.clone())
    };
    let (out, err) = review(&items, home, &s, None, Some(&ok));
    assert!(err.is_none());
    assert_eq!(out[0].safety, Safety::Safe);
    assert_eq!(out[0].reason, "disposable");
    assert_eq!(out[1].safety, Safety::Review);
    assert_eq!(out[2].safety, Safety::Protected);

    let offline = |_: &str, _: &[String], _: &[u8]| Err("offline".to_string());
    let (out, err) = review(&items, home, &s, None, Some(&offline));
    assert!(err.is_some());
    assert_eq!(out[0].safety, Safety::Review, "CLI failure must preserve manual review");
}

#[test]
fn reads_go_json_and_roundtrips_store() {
    let settings_json = r#"{"setupDone":true,"autoScan":true,"openAtLogin":true,"scanHome":true,"roots":["/Users/q/Downloads"],"exclusions":null,"minSizeBytes":500000000,"minAgeDays":90,"redGB":10,"yellowGB":30,"lastScan":"2026-11-07T21:45:20.952217+07:00","lastAlertLevel":"green","lastAlertIds":["1:2"]}"#;
    let s: Settings = serde_json::from_str(settings_json).unwrap();
    assert!(s.scan_home && s.ai_review && s.exclusions.is_empty());
    let zero: Settings = serde_json::from_str(&settings_json.replace("2026-11-07T21:45:20.952217+07:00", "0001-01-01T00:00:00Z")).unwrap();
    assert!(is_zero_time(&zero.last_scan));

    let cand_json = r#"{"id":"1:2","path":"/Users/q/Downloads/a.dmg","size":5,"modifiedAt":"2026-06-12T20:43:23.919696186+07:00","lastUsedAt":"0001-01-01T00:00:00Z","lastUseKnown":false,"device":1,"inode":2,"safety":"review","reason":"r","ignored":false}"#;
    let c: Candidate = serde_json::from_str(cand_json).unwrap();
    assert_eq!(c.modified_at.timestamp_subsec_nanos(), 919_696_186);

    let tmp = tempfile::Builder::new().prefix("sdc").tempdir().unwrap();
    let store = Store::open(tmp.path()).unwrap();
    assert!(!store.load_settings(Path::new("/Users/q")).unwrap().setup_done);
    store.save_settings(&s).unwrap();
    assert_eq!(store.load_settings(Path::new("/Users/q")).unwrap(), s);
    store.replace_candidates(std::slice::from_ref(&c)).unwrap();
    store.ignore(&["1:2".into()]).unwrap();
    let loaded = store.load_candidates().unwrap();
    assert!(loaded[0].ignored);
    assert_eq!(loaded[0].modified_at, c.modified_at);
    store.unignore(&["1:2".into()]).unwrap();
    assert!(!store.load_candidates().unwrap()[0].ignored);
    store.record_action("2026-01-01T00:00:00Z", "trash", "/x", "ok").unwrap();
    assert_eq!(store.recent_actions(10).unwrap().len(), 1);
    store.remove_candidates(&["1:2".into()]).unwrap();
    assert!(store.load_candidates().unwrap().is_empty());
}

#[test]
fn notify_rules() {
    let c = |id: &str, ignored: bool| Candidate {
        id: id.into(),
        path: "/x".into(),
        size: 1,
        modified_at: Utc::now(),
        last_used_at: zero_time(),
        last_use_known: false,
        device: 0,
        inode: 0,
        safety: Safety::Review,
        reason: String::new(),
        ignored,
    };
    assert!(!should_notify(&[c("a", false)], &[c("a", false)], Level::Green, Level::Green));
    assert!(should_notify(&[c("a", false)], &[c("a", false), c("b", false)], Level::Green, Level::Green));
    assert!(!should_notify(&[], &[c("b", true)], Level::Green, Level::Green));
    assert!(should_notify(&[], &[], Level::Green, Level::Yellow));
}

#[test]
fn mdls_dates() {
    assert!(parse_mdls_date("2025-01-02 03:04:05 +0000").is_some());
    assert!(parse_mdls_date("(null)").is_none());
}

#[test]
fn rate_history_buckets() {
    let mut h = RateHistory::new(10);
    let t0 = Instant::now();
    h.push(0, 0, t0);
    h.push(1000, 500, t0 + std::time::Duration::from_secs(1));
    h.push(3000, 500, t0 + std::time::Duration::from_secs(2));
    assert_eq!(h.latest().read, 2000.0);
    let b = h.buckets(4, 2);
    assert_eq!(b.len(), 2);
    assert_eq!(b[0].read, 0.0);
    assert_eq!(b[1].read, 1500.0);
    for i in 3..20 {
        h.push(3000 + i, 500, t0 + std::time::Duration::from_secs(i));
    }
    assert_eq!(h.samples.len(), 10);
}

#[test]
fn more_columns_than_seconds_keeps_latest_at_right() {
    let mut h = RateHistory::new(100);
    let t0 = Instant::now();
    h.push(0, 0, t0);
    h.push(5000, 0, t0 + std::time::Duration::from_secs(1));
    let b = h.buckets(60, 120);
    assert_eq!(b.len(), 120);
    assert_eq!(b[119].read, 5000.0, "newest sample must reach the right edge");
    assert_eq!(b[0].read, 0.0);
}

#[test]
fn groups_helpers_into_app() {
    let (name, app) =
        app_group(Some("/Applications/Google Chrome.app/Contents/Frameworks/Helper.app/Contents/MacOS/Helper"), "Helper");
    assert_eq!(name, "Google Chrome");
    assert_eq!(app.as_deref(), Some("/Applications/Google Chrome.app"));
    assert_eq!(app_group(Some("/usr/libexec/mdworker_shared"), "mdworker_shared").0, "mdworker_shared");
}

#[test]
fn parses_hdiutil_plist() {
    let xml = br#"<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0"><dict><key>images</key><array><dict>
<key>image-path</key><string>/nonexistent/MathStudio.dmg</string>
<key>system-entities</key><array>
<dict><key>content-hint</key><string>GUID_partition_scheme</string><key>dev-entry</key><string>/dev/disk8</string></dict>
<dict><key>content-hint</key><string>Apple_HFS</string><key>dev-entry</key><string>/dev/disk8s1</string><key>mount-point</key><string>/Volumes/MathStudio 1.27.0-arm64</string></dict>
</array></dict></array></dict></plist>"#;
    let imgs = parse_hdiutil(xml);
    assert_eq!(imgs.len(), 1);
    assert_eq!(imgs[0].name, "MathStudio 1.27.0-arm64");
    assert_eq!(imgs[0].fs_type, "hfs");
}

#[test]
fn formats() {
    use crate::format::*;
    assert_eq!(bytes(147_300_000_000), "147 GB");
    assert_eq!(bytes(38_400_000_000), "38.4 GB");
    assert_eq!(bytes(308_000_000), "308 MB");
    assert_eq!(bytes_parts(147_300_000_000), ("147".into(), "GB".into()));
    assert_eq!(rate(92_700_000.0), "92.7 MB/s");
}

#[test]
fn live_counters_are_readable() {
    let (r, w) = crate::sysstats::disk_io_totals();
    assert!(r > 0 || w > 0, "IOKit returned no disk statistics");
    assert!(!crate::sysstats::mounts().is_empty());
}
