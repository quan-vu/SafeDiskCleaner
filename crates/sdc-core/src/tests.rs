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
    for p in ["/Users/test/iCloud Drive (Archive)/a.dmg", "/Users/test/Library/Mobile Documents/x", "/Volumes/X/CloudStorage/a"] {
        assert!(protected(Path::new(p), home, &[]), "not protected: {p}");
    }
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
    assert!(s.scan_home && !s.ai_review && s.exclusions.is_empty());
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
    // CI virtual machines may expose no block-storage statistics.
    if std::env::var_os("CI").is_none() {
        assert!(r > 0 || w > 0, "IOKit returned no disk statistics");
    }
    assert!(!crate::sysstats::mounts().is_empty());
}

fn sample(id: &str, path: &str, size: i64) -> Candidate {
    Candidate {
        id: id.into(),
        path: path.into(),
        size,
        modified_at: zero_time(),
        last_used_at: zero_time(),
        last_use_known: false,
        device: 1,
        inode: 2,
        safety: Safety::Review,
        reason: "fresh".into(),
        ignored: false,
    }
}

#[test]
fn settings_default_interval() {
    let home = Path::new("/Users/test");
    let mut v = serde_json::to_value(Settings::default_for(home)).unwrap();
    v.as_object_mut().unwrap().remove("scanIntervalMinutes");
    let s: Settings = serde_json::from_value(v).unwrap();
    assert_eq!(s.scan_interval_minutes, 10);
    let mut bad = s.clone();
    bad.scan_interval_minutes = 0;
    assert!(bad.validate().is_err());
}

#[test]
fn lists_seeded_once_and_bookmarks() {
    let tmp = tempfile::Builder::new().prefix("sdc").tempdir().unwrap();
    let store = Store::open(tmp.path()).unwrap();
    let lists = store.lists().unwrap();
    assert_eq!(lists.iter().map(|l| l.name.as_str()).collect::<Vec<_>>(), ["App Large Files", "Document Large Files"]);
    store.delete_list(lists[1].id).unwrap();
    drop(store);
    let store = Store::open(tmp.path()).unwrap();
    assert_eq!(store.lists().unwrap().len(), 1, "defaults must not be reseeded");

    let app = store.lists().unwrap()[0].id;
    let docs = store.create_list("Docs").unwrap();
    assert!(store.create_list(" Docs ").is_err());
    assert!(store.create_list("  ").is_err());
    let a = sample("1:1", "/x/a.dmg", 5);
    let b = sample("1:2", "/x/b.pdf", 7);
    store.add_bookmarks(app, &[a.clone(), b.clone()], "t1").unwrap();
    store.add_bookmarks(app, std::slice::from_ref(&a), "t2").unwrap();
    store.add_bookmarks(docs, std::slice::from_ref(&b), "t1").unwrap();
    assert_eq!(store.bookmarks(app).unwrap().len(), 2);
    assert_eq!(store.bookmark_index().unwrap()["1:2"], vec!["App Large Files".to_string(), "Docs".to_string()]);
    store.remove_bookmarks(app, &["1:1".into()]).unwrap();
    assert_eq!(store.bookmarks(app).unwrap(), vec![b.clone()]);
    store.remove_bookmarks_for_files(&["1:2".into()]).unwrap();
    assert!(store.bookmarks(app).unwrap().is_empty());
    assert!(store.bookmarks(docs).unwrap().is_empty());
    store.add_bookmarks(docs, &[a], "t3").unwrap();
    store.rename_list(docs, "Papers").unwrap();
    store.delete_list(docs).unwrap();
    assert!(store.all_bookmarks().unwrap().is_empty(), "delete cascades");
}

#[test]
fn categories() {
    use crate::category::Category;
    assert_eq!(Category::of(Path::new("/a/Xcode.XIP")), Category::App);
    assert_eq!(Category::of(Path::new("/a/setup.dmg")), Category::App);
    assert_eq!(Category::of(Path::new("/a/report.pdf")), Category::Document);
    assert_eq!(Category::of(Path::new("/a/clip.mov")), Category::Video);
    assert_eq!(Category::of(Path::new("/a/backup.tar.gz")), Category::Archive);
    assert_eq!(Category::of(Path::new("/a/noext")), Category::Other);
}

#[test]
fn still_exists_checks_identity() {
    use std::os::unix::fs::MetadataExt;
    let tmp = tempfile::Builder::new().prefix("sdc").tempdir().unwrap();
    let p = tmp.path().join("f.bin");
    std::fs::write(&p, b"x").unwrap();
    let md = std::fs::metadata(&p).unwrap();
    let mut c = sample("x", p.to_str().unwrap(), 1);
    c.device = md.dev() as u64;
    c.inode = md.ino();
    assert!(still_exists(&c));
    std::fs::remove_file(&p).unwrap();
    assert!(!still_exists(&c));
}

#[test]
fn carry_over_keeps_unchanged_verdicts() {
    let mut reviewed = sample("a", "/x/a", 10);
    reviewed.safety = Safety::Safe;
    reviewed.reason = "installer, safe".into();
    let never = sample("b", "/x/b", 10);
    let mut changed = sample("c", "/x/c", 10);
    changed.reason = "old verdict".into();
    let before = vec![reviewed, never, changed];
    let mut after = vec![sample("a", "/x/a", 10), sample("b", "/x/b", 10), sample("c", "/x/c", 11), sample("d", "/x/d", 1)];
    let todo = carry_over(&before, &mut after);
    assert_eq!(todo, vec![1, 2, 3]);
    assert_eq!(after[0].safety, Safety::Safe);
    assert_eq!(after[0].reason, "installer, safe");
    assert_eq!(after[2].reason, "fresh");
}

#[test]
fn snapshots_compare_and_store() {
    use crate::snapshot::{compare, Snapshot};
    let vol = |free| Volume {
        path: "/".into(),
        name: "Macintosh HD".into(),
        device: 1,
        free_bytes: free,
        total_bytes: 100,
        fs_type: "APFS".into(),
        bsd_name: "disk3s1".into(),
        level: Level::Green,
    };
    let s1 = Snapshot::new(
        "2026-11-15",
        "t",
        &[vol(60)],
        &[("/d/A".into(), 10), ("/d/B".into(), 5)],
        &[sample("1", "/d/A/old.dmg", 9)],
    );
    let s2 = Snapshot::new(
        "2026-11-16",
        "t",
        &[vol(40)],
        &[("/d/A".into(), 30), ("/d/C".into(), 1)],
        &[sample("2", "/d/A/new.mov", 20)],
    );
    let c = compare(&s1, &s2);
    assert_eq!(c.used_delta, 20);
    assert_eq!(c.folders, vec![("/d/A".into(), 10, 30), ("/d/B".into(), 5, 0), ("/d/C".into(), 0, 1)]);
    assert_eq!(c.new_files[0].id, "2");
    assert_eq!(c.gone_files[0].id, "1");

    let tmp = tempfile::Builder::new().prefix("sdc").tempdir().unwrap();
    let store = Store::open(tmp.path()).unwrap();
    store.save_snapshot(&s1, 2).unwrap();
    store.save_snapshot(&s2, 2).unwrap();
    store.save_snapshot(&s2, 2).unwrap();
    let mut s3 = s2.clone();
    s3.day = "2026-11-17".into();
    store.save_snapshot(&s3, 2).unwrap();
    let days: Vec<String> = store.snapshots(10).unwrap().into_iter().map(|s| s.day).collect();
    assert_eq!(days, ["2026-11-17", "2026-11-16"]);
}

#[test]
fn scan_reports_folder_sizes() {
    let tmp = tempfile::Builder::new().prefix("sdc").tempdir().unwrap();
    let home = tmp.path().canonicalize().unwrap();
    let root = home.join("Downloads");
    std::fs::create_dir_all(root.join("sub/deep")).unwrap();
    std::fs::write(root.join("sub/deep/a.bin"), vec![1u8; 10_000]).unwrap();
    std::fs::write(root.join("top.bin"), vec![1u8; 10_000]).unwrap();
    let s = Settings::default_for(&home);
    let none = |_: &Path| None;
    let r = scan(&home, &s, Some(&none), &AtomicBool::new(false)).unwrap();
    let get = |p: &Path| r.folder_sizes.iter().find(|(k, _)| Path::new(k) == p).map(|(_, v)| *v).unwrap_or(0);
    assert!(get(&root.join("sub")) >= 10_000, "{:?}", r.folder_sizes);
    assert!(get(&root) >= 10_000, "{:?}", r.folder_sizes);
}

#[test]
fn scan_reports_progress_and_log() {
    use crate::scan::{scan_with_progress, Progress};
    use std::sync::atomic::Ordering;
    let tmp = tempfile::Builder::new().prefix("sdc").tempdir().unwrap();
    let home = tmp.path().canonicalize().unwrap();
    let root = home.join("Downloads");
    std::fs::create_dir_all(root.join("sub")).unwrap();
    std::fs::write(root.join("sub/big.dmg"), b"123456").unwrap();
    filetime::set_file_mtime(root.join("sub/big.dmg"), filetime::FileTime::from_unix_time(1_000_000_000, 0)).unwrap();
    std::fs::create_dir_all(root.join("repo/.git")).unwrap();
    let mut s = Settings::default_for(&home);
    s.min_size_bytes = 4;
    let none = |_: &Path| None;
    let p = Progress::default();
    let r = scan_with_progress(&home, &s, Some(&none), &AtomicBool::new(false), &p).unwrap();
    assert_eq!(r.candidates.len(), 1);
    assert!(p.dirs.load(Ordering::Relaxed) >= 2);
    assert_eq!(p.found.load(Ordering::Relaxed), 1);
    assert_eq!(p.lookups_done.load(Ordering::Relaxed), 1);
    let log = p.take_log().join("\n");
    assert!(log.contains("▶ Scanning folder ~/Downloads"), "{log}");
    assert!(log.contains("~/Downloads/sub"), "{log}");
    assert!(log.contains("Found") && log.contains("big.dmg"), "{log}");
    assert!(log.contains("git repository"), "{log}");
    assert!(p.take_log().is_empty(), "log drains");
}

#[test]
fn overview_pack_and_normalize() {
    use crate::overview::*;
    let (p, rows) = pack(&[CardSize::L, CardSize::M, CardSize::S, CardSize::S], 4);
    assert_eq!(rows, 2);
    assert_eq!((p[0].col, p[0].row), (0, 0));
    assert_eq!((p[1].col, p[1].row), (2, 0));
    assert_eq!((p[2].col, p[2].row), (2, 1));
    assert_eq!((p[3].col, p[3].row), (3, 1));
    // An S first leaves a hole that a later S fills (dense placement).
    let (p, rows) = pack(&[CardSize::S, CardSize::L, CardSize::L, CardSize::S], 4);
    assert_eq!((p[1].col, p[2].col, p[2].row), (1, 0, 2));
    assert_eq!((p[3].col, p[3].row), (3, 0));
    assert_eq!(rows, 4);
    let n = normalize(&[CardPref { id: "cpu".into(), size: CardSize::L }, CardPref { id: "bogus".into(), size: CardSize::S }, CardPref { id: "cpu".into(), size: CardSize::S }]);
    assert_eq!(n.iter().map(|c| c.id.as_str()).collect::<Vec<_>>(), ["cpu", "disk", "temp", "ram"]);
    assert_eq!(n[0].size, CardSize::L);
}

#[test]
fn system_metrics_are_sane() {
    use crate::sysstats::*;
    let m = memory().expect("memory stats");
    assert!(m.total > 0 && m.used() <= m.total);
    assert!(cpu_cores() >= 1);
    let mut c = CpuSampler::default();
    assert!(c.sample().is_none());
    std::thread::sleep(std::time::Duration::from_millis(50));
    let v = c.sample().expect("second sample");
    assert!((0.0..=100.0).contains(&v));
    let tmp = tempfile::Builder::new().prefix("sdc").tempdir().unwrap();
    std::fs::write(tmp.path().join("a"), vec![0u8; 8192]).unwrap();
    assert!(dir_size(tmp.path()) >= 8192);
    assert_eq!(dir_size(&tmp.path().join("missing")), 0);
}

#[test]
fn folder_usage_measure_fold_store() {
    use crate::usage::*;
    let tmp = tempfile::Builder::new().prefix("sdc").tempdir().unwrap();
    let home = tmp.path().canonicalize().unwrap();
    std::fs::create_dir_all(home.join("Big/sub")).unwrap();
    std::fs::write(home.join("Big/sub/a.bin"), vec![1u8; 200_000]).unwrap();
    std::fs::create_dir_all(home.join("Small")).unwrap();
    std::fs::write(home.join("Small/b.bin"), vec![1u8; 20_000]).unwrap();
    std::fs::create_dir_all(home.join("Library/Containers/x")).unwrap();
    std::fs::write(home.join("Library/Containers/x/c.bin"), vec![1u8; 500_000]).unwrap();
    std::fs::write(home.join("Library/d.bin"), vec![1u8; 10_000]).unwrap();
    std::fs::write(home.join("loose.txt"), vec![1u8; 5_000]).unwrap();
    std::os::unix::fs::symlink(home.join("Big"), home.join("Link")).unwrap();
    let seen = std::cell::RefCell::new(vec![]);
    std::fs::create_dir_all(home.join("Library/Mobile Documents/com~apple~CloudDocs")).unwrap();
    std::fs::write(home.join("Library/Mobile Documents/com~apple~CloudDocs/e.bin"), vec![1u8; 900_000]).unwrap();
    std::fs::create_dir_all(home.join("iCloud Drive (Archive)")).unwrap();
    std::fs::write(home.join("iCloud Drive (Archive)/f.bin"), vec![1u8; 300_000]).unwrap();
    std::fs::create_dir_all(home.join("Desktop")).unwrap();
    std::fs::write(home.join("Desktop/g.bin"), vec![1u8; 50_000]).unwrap();
    let with_cloud = home_usage(&home, true, &[], "t", &|_| {}, &|| false);
    assert!(with_cloud.parts.iter().any(|p| p.label == "Desktop"));
    assert!(with_cloud.parts.iter().any(|p| p.label == "iCloud Drive (Archive)"));
    let lib_cloud = with_cloud.parts.iter().find(|p| p.label == "Library").unwrap().bytes;
    assert!(lib_cloud >= 900_000, "include_icloud walks Mobile Documents: {lib_cloud}");
    let blocked = Settings::default_for(&home).blocked_private(&home);
    assert_eq!(blocked, vec![home.join("Desktop")]);
    let u = home_usage(&home, false, &blocked, "t", &|p| seen.borrow_mut().push(p.to_string()), &|| false);
    assert!(!u.parts.iter().any(|p| p.label == "Desktop"), "Desktop is not opened by default");
    assert!(!seen.borrow().iter().any(|p| p.contains("Desktop")));
    assert!(!u.parts.iter().any(|p| p.label.starts_with("iCloud")), "iCloud folders skipped by default");
    assert!(!seen.borrow().iter().any(|p| p.contains("iCloud")));
    let labels: Vec<&str> = u.parts.iter().map(|p| p.label.as_str()).collect();
    assert_eq!(labels[0], "Big");
    assert!(labels.contains(&"Small") && labels.contains(&"Files in Home") && !labels.contains(&"Link"));
    let lib = u.parts.iter().find(|p| p.label == "Library").unwrap();
    assert!(lib.bytes < 100_000, "app containers are skipped: {}", lib.bytes);
    assert!(seen.borrow().contains(&"~/Big".to_string()));

    let d = disk_parts(&home, 3_000, 1_000, 500, 10_000, 4_000, "t");
    let got: Vec<(&str, i64)> = d.parts.iter().map(|p| (p.label.as_str(), p.bytes)).collect();
    assert_eq!(got, [("Home", 3_000), ("System & other", 1_500), ("Applications", 1_000), ("Library", 500), ("Free", 4_000)]);
    assert_eq!(d.total(), 10_000);

    let parts: Vec<Part> = (0..10).map(|i| Part { label: format!("p{i}"), path: String::new(), bytes: 100 - i }).collect();
    let mut with_free = parts.clone();
    with_free.push(Part { label: "Free".into(), path: String::new(), bytes: 1 });
    let f = fold(&with_free, 8, &["Free"]);
    assert_eq!(f.len(), 8);
    assert_eq!(f[6].label, "Other");
    assert_eq!(f[6].bytes, (91..=94).sum::<i64>(), "p6..p9 fold into Other");
    assert_eq!(f[7].label, "Free");
    assert_eq!(fold(&parts[..3], 8, &[]).len(), 3);

    let store = Store::open(&home.join("db")).unwrap();
    assert!(store.load_usage("home").unwrap().is_none());
    store.save_usage("home", &u).unwrap();
    assert_eq!(store.load_usage("home").unwrap().unwrap(), u);
}

#[test]
fn scan_never_opens_disallowed_private_folders() {
    let tmp = tempfile::Builder::new().prefix("sdc").tempdir().unwrap();
    let home = tmp.path().canonicalize().unwrap();
    for d in ["Desktop", "Documents", "Downloads"] {
        std::fs::create_dir_all(home.join(d)).unwrap();
        let f = home.join(d).join("old.dmg");
        std::fs::write(&f, b"123456").unwrap();
        filetime::set_file_mtime(&f, filetime::FileTime::from_unix_time(1_000_000_000, 0)).unwrap();
    }
    let mut s = Settings::default_for(&home);
    s.scan_home = true;
    s.min_size_bytes = 4;
    let none = |_: &Path| None;
    let p = crate::scan::Progress::default();
    let r = crate::scan::scan_with_progress(&home, &s, Some(&none), &AtomicBool::new(false), &p).unwrap();
    let mut found: Vec<String> = r.candidates.iter().map(|c| Path::new(&c.path).parent().unwrap().file_name().unwrap().to_string_lossy().into_owned()).collect();
    found.sort();
    assert_eq!(found, ["Documents", "Downloads"]);
    assert!(p.take_log().join("\n").contains("Skipped ~/Desktop (no access"));
    // A root inside a disallowed folder is skipped too.
    s.scan_home = false;
    s.roots = vec![home.join("Desktop").to_string_lossy().into_owned()];
    let r = scan(&home, &s, Some(&none), &AtomicBool::new(false)).unwrap();
    assert!(r.candidates.is_empty());
    // Allowing it opens it.
    s.private_folders.push("Desktop".into());
    let r = scan(&home, &s, Some(&none), &AtomicBool::new(false)).unwrap();
    assert_eq!(r.candidates.len(), 1);
}
