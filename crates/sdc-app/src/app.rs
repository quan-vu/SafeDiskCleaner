//! Application state and actions. Everything here runs on the main thread;
//! slow work (scan, AI review, sampling) runs on worker threads and hands its
//! result back through the main dispatch queue.

use crate::system;
use crate::ui::*;
use dispatch2::DispatchQueue;
use objc2::rc::Retained;
use objc2::MainThreadMarker;
use objc2_app_kit::*;
use objc2_foundation::*;
use sdc_core::action::{delete_permanently, validate_for_action};
use sdc_core::format;
use sdc_core::model::*;
use sdc_core::review;
use sdc_core::scan::{scan_with_progress, volume_stats, Progress};
use sdc_core::category::Category;
use sdc_core::snapshot::Snapshot;
use sdc_core::store::{ListInfo, Store};
use sdc_core::usage::Usage;
use sdc_core::overview::{normalize, CardPref, CardSize};
use sdc_core::sysstats::{self, CpuSampler, DiskImage, MemInfo, Mount, ProcIo, ProcSampler, RateHistory, TempSizes};
use std::cell::{Cell, OnceCell, RefCell};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

static VI: AtomicBool = AtomicBool::new(false);

/// Pick the Vietnamese or English string for the user's language.
pub fn tr(vi: &str, en: &str) -> String {
    if VI.load(Ordering::Relaxed) { vi.to_string() } else { en.to_string() }
}

thread_local! {
    static APP: OnceCell<Rc<App>> = const { OnceCell::new() };
}

pub fn app() -> Rc<App> {
    APP.with(|a| a.get().expect("app not initialised").clone())
}

/// Run `f` on the main thread later (never inside the current call stack).
pub fn later(f: impl FnOnce(&App) + Send + 'static) {
    DispatchQueue::main().exec_async(move || f(&app()));
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Page {
    Overview,
    Files,
    Disk,
    Volumes,
    Ignored,
    History,
    Settings,
    /// Day-by-day disk usage changes.
    Changes,
    /// Space used per folder (home / disk), as a pie or columns.
    Storage,
    /// A user bookmark list.
    List(i64),
}

/// Days of snapshots kept in the database.
const SNAPSHOT_DAYS: usize = 90;

#[derive(Default)]
pub struct Live {
    pub history: Option<RateHistory>,
    pub procs: Vec<ProcIo>,
    pub images: Vec<DiskImage>,
    pub mounts: Vec<Mount>,
    pub machine: String,
    /// CPU load history in percent, oldest first (last 2 minutes).
    pub cpu: std::collections::VecDeque<f64>,
    pub cores: usize,
    pub mem: Option<MemInfo>,
    pub temp: Option<TempSizes>,
    /// Bumped whenever `temp` is refreshed.
    pub temp_rev: u64,
}

/// CPU samples kept (one per second).
const CPU_HISTORY: usize = 120;

pub struct NavItem {
    pub page: Page,
    pub row: Retained<ClickView>,
    pub title: Retained<NSTextField>,
    pub metric: Retained<NSTextField>,
    pub metric_color: Retained<NSColor>,
}

#[derive(Default)]
pub struct Refs {
    pub rate_total: Option<Retained<NSTextField>>,
    pub rate_read: Option<Retained<NSTextField>>,
    pub rate_write: Option<Retained<NSTextField>>,
    pub read_total: Option<Retained<NSTextField>>,
    pub written_total: Option<Retained<NSTextField>>,
    pub chart: Option<Retained<DotChart>>,
    pub procs: Option<Retained<NSStackView>>,
    pub rows: Vec<(String, Retained<ClickView>, Retained<NSButton>)>,
    pub sel_label: Option<Retained<NSTextField>>,
    pub sel_buttons: Vec<Retained<NSButton>>,
    pub fields: Vec<Retained<NSTextField>>,
    pub exclusions: Option<Retained<NSTextView>>,
    /// Overview live values: (label, hero size).
    pub cpu_value: Option<(Retained<NSTextField>, f64)>,
    pub cpu_spark: Option<Retained<Spark>>,
    /// Average and peak labels of the large CPU card.
    pub cpu_stats: Vec<Retained<NSTextField>>,
    pub mem_value: Option<(Retained<NSTextField>, f64)>,
    pub mem_bar: Option<Retained<Bar>>,
    /// (part: 0 app, 1 wired, 2 compressed, 3 available; label, with name prefix)
    pub mem_labels: Vec<(u8, Retained<NSTextField>, bool)>,
    /// Storage page: "Measuring ~/Library…" while folder sizes are measured.
    pub usage_status: Option<Retained<NSTextField>>,
}

pub struct App {
    pub mtm: MainThreadMarker,
    pub home: PathBuf,
    pub store: Arc<Store>,
    pub settings: RefCell<Settings>,
    pub candidates: RefCell<Vec<Candidate>>,
    pub volumes: RefCell<Vec<Volume>>,
    pub selected: RefCell<HashSet<String>>,
    pub page: Cell<Page>,
    pub tab: Cell<isize>,
    pub filter: RefCell<String>,
    pub sort_small: Cell<bool>,
    pub scanning: Cell<bool>,
    /// Cancel flag of the running scan (a fresh one per scan).
    pub cancel: RefCell<Arc<AtomicBool>>,
    /// Bumped per scan; results of an abandoned scan are ignored.
    pub scan_gen: Cell<u64>,
    pub progress: RefCell<Option<Arc<Progress>>>,
    pub scan_started: Cell<Option<Instant>>,
    pub sheet: RefCell<Option<crate::scan_sheet::ScanSheet>>,
    pub status: RefCell<String>,
    pub range: Cell<isize>,
    pub paused: Cell<bool>,
    pub live: Arc<Mutex<Live>>,
    pub window: RefCell<Option<Retained<NSWindow>>>,
    pub sidebar: RefCell<Option<Retained<NSView>>>,
    pub main_area: RefCell<Option<Retained<NSView>>>,
    pub nav: RefCell<Vec<NavItem>>,
    pub footer_status: RefCell<Option<Retained<NSTextField>>>,
    pub footer_machine: RefCell<Option<Retained<NSTextField>>>,
    pub page_targets: RefCell<Vec<Retained<Target>>>,
    pub keep: RefCell<Vec<Retained<Target>>>,
    pub refs: RefCell<Refs>,
    pub status_item: RefCell<Option<Retained<NSStatusItem>>>,
    pub draft: RefCell<Option<Settings>>,
    pub draft_text: RefCell<Option<[String; 5]>>,
    pub setup_home: Cell<bool>,
    render_pending: Cell<bool>,
    /// User collapsed the sidebar with the toolbar toggle.
    pub sidebar_collapsed: Cell<bool>,
    /// (main after sidebar, main at window edge) leading constraints.
    pub main_leading: RefCell<Option<(Retained<NSLayoutConstraint>, Retained<NSLayoutConstraint>)>>,
    /// Bookmark lists with counts, in sidebar order.
    pub lists: RefCell<Vec<ListInfo>>,
    /// Files of the list page being shown.
    pub list_items: RefCell<Vec<Candidate>>,
    /// file id -> names of lists holding it.
    pub bookmark_index: RefCell<HashMap<String, Vec<String>>>,
    pub category_filter: Cell<Option<Category>>,
    /// Sidebar container refilled with one row per list.
    pub lists_box: RefCell<Option<Retained<NSStackView>>>,
    pruning: Cell<bool>,
    /// Snapshot index (newest = 0) whose changes the Daily changes page details.
    pub changes_day: Cell<usize>,
    /// Overview is in "arrange cards" mode.
    pub arranging: Cell<bool>,
    ticks: Cell<u64>,
    temp_seen: Cell<u64>,
    /// Live value lines in the tray menu: (metric id, item).
    pub tray_lines: RefCell<Vec<(&'static str, Retained<NSMenuItem>)>>,
    /// Last folder-size measurements ("home" and "disk").
    pub usage_home: RefCell<Option<Usage>>,
    pub usage_disk: RefCell<Option<Usage>>,
    /// Storage page: 0 = Home, 1 = Disk.
    pub usage_scope: Cell<isize>,
    /// Storage page: 0 = Pie, 1 = Columns.
    pub usage_chart: Cell<isize>,
    pub usage_measuring: Cell<bool>,
    usage_progress: Arc<Mutex<String>>,
    usage_cancel: Arc<AtomicBool>,
}

pub struct ScanOutcome {
    result: Result<(Vec<Candidate>, Vec<Volume>, usize, Settings), String>,
    ai_error: Option<String>,
    before: Vec<Candidate>,
    before_level: Level,
}

impl App {
    pub fn init(mtm: MainThreadMarker) -> Result<Rc<App>, String> {
        VI.store(system::preferred_vietnamese(), Ordering::Relaxed);
        let home = PathBuf::from(std::env::var("HOME").map_err(|e| e.to_string())?);
        let dir = home.join("Library/Application Support/Safety Disk Cleaner");
        let store = Store::open(&dir)?;
        let settings = store.load_settings(&home)?;
        let candidates = store.load_candidates()?;
        let lists = store.lists()?;
        let bookmark_index = store.bookmark_index()?;
        let usage_home = store.load_usage("home").unwrap_or(None);
        let usage_disk = store.load_usage("disk").unwrap_or(None);
        let a = Rc::new(App {
            mtm,
            home,
            store: Arc::new(store),
            settings: RefCell::new(settings),
            candidates: RefCell::new(candidates),
            volumes: RefCell::new(vec![]),
            selected: RefCell::new(HashSet::new()),
            page: Cell::new(Page::Overview),
            tab: Cell::new(0),
            filter: RefCell::new(String::new()),
            sort_small: Cell::new(false),
            scanning: Cell::new(false),
            cancel: RefCell::new(Arc::new(AtomicBool::new(false))),
            scan_gen: Cell::new(0),
            progress: RefCell::new(None),
            scan_started: Cell::new(None),
            sheet: RefCell::new(None),
            status: RefCell::new(String::new()),
            range: Cell::new(0),
            paused: Cell::new(false),
            live: Arc::new(Mutex::new(Live { history: Some(RateHistory::new(3600)), ..Default::default() })),
            window: RefCell::new(None),
            sidebar: RefCell::new(None),
            main_area: RefCell::new(None),
            nav: RefCell::new(vec![]),
            footer_status: RefCell::new(None),
            footer_machine: RefCell::new(None),
            page_targets: RefCell::new(vec![]),
            keep: RefCell::new(vec![]),
            refs: RefCell::new(Refs::default()),
            status_item: RefCell::new(None),
            draft: RefCell::new(None),
            draft_text: RefCell::new(None),
            setup_home: Cell::new(false),
            render_pending: Cell::new(false),
            sidebar_collapsed: Cell::new(false),
            main_leading: RefCell::new(None),
            lists: RefCell::new(lists),
            list_items: RefCell::new(vec![]),
            bookmark_index: RefCell::new(bookmark_index),
            category_filter: Cell::new(None),
            lists_box: RefCell::new(None),
            pruning: Cell::new(false),
            changes_day: Cell::new(0),
            arranging: Cell::new(false),
            ticks: Cell::new(0),
            temp_seen: Cell::new(0),
            tray_lines: RefCell::new(vec![]),
            usage_home: RefCell::new(usage_home),
            usage_disk: RefCell::new(usage_disk),
            usage_scope: Cell::new(0),
            usage_chart: Cell::new(0),
            usage_measuring: Cell::new(false),
            usage_progress: Arc::new(Mutex::new(String::new())),
            usage_cancel: Arc::new(AtomicBool::new(false)),
        });
        APP.with(|cell| {
            let _ = cell.set(a.clone());
        });
        Ok(a)
    }

    // ------------------------------------------------------------ targets & render

    /// Target for a control on the current page (released on the next render).
    pub fn target(&self, f: impl Fn(&App) + 'static) -> Retained<Target> {
        let t = Target::new(self.mtm, move |_| f(&app()));
        self.page_targets.borrow_mut().push(t.clone());
        t
    }

    /// Target that lives for the whole session (sidebar, tray).
    pub fn keep_target(&self, f: impl Fn(&App) + 'static) -> Retained<Target> {
        let t = Target::new(self.mtm, move |_| f(&app()));
        self.keep.borrow_mut().push(t.clone());
        t
    }

    /// Re-render the current page after the current event finishes. Never
    /// rebuild synchronously: the control that triggered us would be freed
    /// while its own action is still running.
    pub fn request_render(&self) {
        if self.render_pending.replace(true) {
            return;
        }
        later(|a| {
            a.render_pending.set(false);
            crate::pages::render(a);
        });
    }

    pub fn navigate(&self, page: Page) {
        debug(&format!("navigate: {page:?}"));
        if self.page.get() == Page::Settings && page != Page::Settings {
            *self.draft.borrow_mut() = None;
            *self.draft_text.borrow_mut() = None;
        }
        if self.page.get() != page {
            self.selected.borrow_mut().clear();
        }
        self.page.set(page);
        self.status.borrow_mut().clear();
        if page == Page::Changes {
            self.changes_day.set(0);
        }
        self.highlight_nav();
        if matches!(page, Page::Files | Page::Ignored | Page::List(_)) {
            self.prune_missing();
        }
        if page == Page::Storage && self.usage_stale() {
            self.measure_usage();
        }
        self.request_render();
    }

    /// Paint the sidebar row of the current page as selected.
    pub fn highlight_nav(&self) {
        let page = self.page.get();
        for item in self.nav.borrow().iter() {
            let on = item.page == page;
            item.row.set_selected(on);
            item.title.setTextColor(Some(&*if on { NSColor::whiteColor() } else { Palette::primary() }));
            item.metric.setTextColor(Some(&*if on { NSColor::whiteColor() } else { item.metric_color.clone() }));
        }
    }

    pub fn toggle_sidebar(&self) {
        self.sidebar_collapsed.set(!self.sidebar_collapsed.get());
        self.request_render();
    }

    pub fn show_window(&self) {
        if let Some(w) = self.window.borrow().as_ref() {
            w.makeKeyAndOrderFront(None);
            #[allow(deprecated)]
            NSApplication::sharedApplication(self.mtm).activateIgnoringOtherApps(true);
        }
        system::clear_notifications();
    }

    pub fn set_status(&self, s: impl Into<String>) {
        *self.status.borrow_mut() = s.into();
        self.request_render();
    }

    // ------------------------------------------------------------ derived data

    pub fn active(&self) -> Vec<Candidate> {
        self.candidates.borrow().iter().filter(|c| !c.ignored).cloned().collect()
    }

    pub fn reclaimable(&self) -> (i64, usize, i64, usize) {
        let (mut safe, mut ns, mut review, mut nr) = (0, 0, 0, 0);
        for c in self.candidates.borrow().iter().filter(|c| !c.ignored) {
            if c.safety == Safety::Safe {
                safe += c.size;
                ns += 1;
            } else {
                review += c.size;
                nr += 1;
            }
        }
        (safe, ns, review, nr)
    }

    /// Files the current page lists: a bookmark list or the scan results.
    pub fn page_items(&self) -> Vec<Candidate> {
        match self.page.get() {
            Page::List(_) => self.list_items.borrow().clone(),
            _ => self.candidates.borrow().clone(),
        }
    }

    pub fn selected_items(&self) -> Vec<Candidate> {
        let sel = self.selected.borrow();
        self.page_items().into_iter().filter(|c| sel.contains(&c.id)).collect()
    }

    pub fn worst(&self) -> Level {
        worst_level(&self.volumes.borrow())
    }

    // ------------------------------------------------------------ volumes, tray

    pub fn refresh_volumes(&self) {
        let s = self.settings.borrow().clone();
        let mut seen = HashSet::new();
        let mut vols = vec![];
        for root in s.effective_roots(&self.home) {
            if let Ok(v) = volume_stats(&root, &s) {
                if seen.insert(v.device) {
                    vols.push(v);
                }
            }
        }
        *self.volumes.borrow_mut() = vols;
        self.update_tray();
    }

    pub fn update_tray(&self) {
        let Some(item) = self.status_item.borrow().clone() else { return };
        crate::pages::fill_tray_menu(self, &item);
        self.update_tray_title();
    }

    // ------------------------------------------------------------ scanning

    pub fn maybe_scan(&self) {
        let s = self.settings.borrow().clone();
        let due = is_zero_time(&s.last_scan) || chrono::Utc::now() - s.last_scan >= chrono::Duration::minutes(s.scan_interval_minutes);
        if s.setup_done && s.auto_scan && !self.scanning.get() && due {
            self.start_scan();
        }
    }

    pub fn start_scan(&self) {
        if self.scanning.get() || !self.settings.borrow().setup_done {
            return;
        }
        self.scanning.set(true);
        let cancel = Arc::new(AtomicBool::new(false));
        *self.cancel.borrow_mut() = cancel.clone();
        let gen = self.scan_gen.get() + 1;
        self.scan_gen.set(gen);
        let progress = Arc::new(Progress::default());
        *self.progress.borrow_mut() = Some(progress.clone());
        self.scan_started.set(Some(Instant::now()));
        progress.log(format!("Scan started {}", chrono::Local::now().format("%Y-%m-%d %H:%M:%S")));
        self.set_status(tr("Đang quét…", "Scanning…"));
        let settings = self.settings.borrow().clone();
        let before = self.candidates.borrow().clone();
        let before_for_review = before.clone();
        let before_level = Level::parse(&settings.last_alert_level);
        let home = self.home.clone();
        let store = self.store.clone();
        std::thread::spawn(move || {
            let mut ai_error = None;
            let result = (|| {
                let r = scan_with_progress(&home, &settings, None, &cancel, &progress).map_err(|e| e.to_string())?;
                let mut cands = r.candidates;
                if settings.ai_review {
                    // Unchanged files keep their verdict; only new or changed ones go to Claude.
                    let todo = carry_over(&before_for_review, &mut cands);
                    if !todo.is_empty() {
                        match review::find_claude() {
                            Some(bin) => {
                                progress.set_phase(tr("Đang nhờ Claude đánh giá…", "Asking Claude to review…"));
                                progress.log(format!("▶ Asking Claude to review {} files", todo.len()));
                                let subset: Vec<Candidate> = todo.iter().map(|&i| cands[i].clone()).collect();
                                let (reviewed, err) = review::review(&subset, &home, &settings, Some(&bin), None);
                                for (&i, c) in todo.iter().zip(reviewed) {
                                    cands[i] = c;
                                }
                                ai_error = err;
                            }
                            None => ai_error = Some("claude CLI not found".into()),
                        }
                    }
                }
                progress.set_phase(tr("Đang lưu kết quả…", "Saving results…"));
                store.replace_candidates(&cands)?;
                let saved = store.load_candidates()?;
                let now = chrono::Local::now();
                let active: Vec<Candidate> = saved.iter().filter(|c| !c.ignored).cloned().collect();
                let snap = Snapshot::new(
                    &now.format("%Y-%m-%d").to_string(),
                    &now.to_rfc3339_opts(chrono::SecondsFormat::Secs, false),
                    &r.volumes,
                    &r.folder_sizes,
                    &active,
                );
                store.save_snapshot(&snap, SNAPSHOT_DAYS)?;
                let mut s = settings.clone();
                s.last_scan = chrono::Utc::now();
                s.last_alert_ids = candidate_ids(&saved);
                s.last_alert_level = worst_level(&r.volumes).as_str().to_string();
                store.save_settings(&s)?;
                Ok((saved, r.volumes, r.skipped, s))
            })();
            let outcome = ScanOutcome { result, ai_error, before, before_level };
            later(move |a| a.finish_scan(gen, outcome));
        });
    }

    /// Stop the running scan. A scan stuck inside a folder read (for example
    /// behind a macOS permission prompt) cannot notice the cancel flag, so it
    /// is abandoned: its result will be ignored when it eventually returns.
    pub fn stop_scan(&self) {
        if !self.scanning.get() {
            return;
        }
        self.cancel.borrow().store(true, Ordering::Relaxed);
        let progress = self.progress.borrow().clone();
        if let Some(p) = &progress {
            p.log("■ Stopping…");
        }
        if progress.is_some_and(|p| p.stalled_for() >= Duration::from_secs(2)) {
            self.scan_gen.set(self.scan_gen.get() + 1);
            self.scanning.set(false);
            if let Some(p) = self.progress.borrow().as_ref() {
                p.log("■ Scan stopped. The blocked folder read will finish in the background; its result is discarded.");
            }
            // Keep showing the stopped scan's log under the new generation.
            if let Some(s) = self.sheet.borrow_mut().as_mut() {
                s.keep_log_for(self.scan_gen.get());
            }
            self.set_status(tr("Đã dừng quét.", "Scan stopped."));
        }
    }

    fn finish_scan(&self, gen: u64, o: ScanOutcome) {
        if gen != self.scan_gen.get() {
            debug("ignoring result of an abandoned scan");
            return;
        }
        self.scanning.set(false);
        let progress = self.progress.borrow().clone();
        let log = |line: String| {
            if let Some(p) = &progress {
                p.log(line);
            }
        };
        match &o.result {
            Err(e) if e == "scan cancelled" => log("■ Scan stopped.".into()),
            Err(e) => log(format!("✗ Scan failed: {e}")),
            Ok((saved, _, skipped, _)) => log(format!(
                "✓ Scan complete in {}s: {} suggestions, {skipped} skipped.",
                self.scan_started.get().map(|t| t.elapsed().as_secs()).unwrap_or(0),
                saved.iter().filter(|c| !c.ignored).count()
            )),
        }
        if let Some(e) = &o.ai_error {
            log(format!("⚠ Claude: {e}"));
        }
        match o.result {
            Err(e) if e == "scan cancelled" => self.set_status(tr("Đã dừng quét.", "Scan stopped.")),
            Err(e) => self.set_status(e),
            Ok((saved, volumes, skipped, s)) => {
                let after_level = worst_level(&volumes);
                let notify = should_notify(&o.before, &saved, o.before_level, after_level);
                let count = saved.iter().filter(|c| !c.ignored).count();
                *self.candidates.borrow_mut() = saved;
                if !volumes.is_empty() {
                    *self.volumes.borrow_mut() = volumes;
                }
                *self.settings.borrow_mut() = s;
                self.selected.borrow_mut().clear();
                self.update_tray();
                let mut msg = tr(
                    &format!("Đã quét: {count} gợi ý, bỏ qua {skipped} mục."),
                    &format!("Scan complete: {count} suggestions, {skipped} skipped."),
                );
                if let Some(e) = o.ai_error {
                    msg.push_str(&format!(" Claude: {e}"));
                }
                if notify && count > 0 {
                    system::notify(
                        "Safety Disk Cleaner",
                        &tr(&format!("Có {count} tệp cần xem trước khi dọn."), &format!("{count} files to review for cleaning.")),
                    );
                }
                self.update_sidebar_counts();
                self.set_status(msg);
            }
        }
    }

    // ------------------------------------------------------------ selection & actions

    pub fn toggle(&self, id: &str) {
        debug(&format!("toggle: {id}"));
        {
            let mut sel = self.selected.borrow_mut();
            if !sel.remove(id) {
                sel.insert(id.to_string());
            }
        }
        self.sync_selection();
    }

    pub fn set_all(&self, ids: &[String], on: bool) {
        {
            let mut sel = self.selected.borrow_mut();
            for id in ids {
                if on {
                    sel.insert(id.clone());
                } else {
                    sel.remove(id);
                }
            }
        }
        self.sync_selection();
    }

    /// Update row highlights and the action bar without rebuilding the list.
    pub fn sync_selection(&self) {
        let sel = self.selected.borrow();
        let refs = self.refs.borrow();
        for (id, row, check) in &refs.rows {
            let on = sel.contains(id);
            row.set_selected(on);
            check.setState(if on { NSControlStateValueOn } else { NSControlStateValueOff });
        }
        let items: Vec<Candidate> = self.page_items().into_iter().filter(|c| sel.contains(&c.id)).collect();
        let total: i64 = items.iter().map(|c| c.size).sum();
        if let Some(l) = refs.sel_label.as_ref() {
            l.setStringValue(&ns(&if items.is_empty() {
                tr("Chưa chọn tệp nào", "No files selected")
            } else {
                tr(
                    &format!("Đã chọn {} · {}", items.len(), format::bytes(total)),
                    &format!("{} selected · {}", items.len(), format::bytes(total)),
                )
            }));
        }
        for b in &refs.sel_buttons {
            b.setEnabled(!items.is_empty());
        }
    }

    pub fn reveal_selected(&self) {
        let paths: Vec<String> = self.selected_items().into_iter().map(|c| c.path).collect();
        if !paths.is_empty() {
            system::reveal(&paths);
        }
    }

    pub fn ignore_selected(&self, ignore: bool) {
        let ids: Vec<String> = self.selected_items().into_iter().filter(|c| c.ignored != ignore).map(|c| c.id).collect();
        if ids.is_empty() {
            return;
        }
        let r = if ignore { self.store.ignore(&ids) } else { self.store.unignore(&ids) };
        if let Err(e) = r {
            return self.set_status(e);
        }
        for c in self.candidates.borrow_mut().iter_mut() {
            if ids.contains(&c.id) {
                c.ignored = ignore;
            }
        }
        self.selected.borrow_mut().clear();
        self.update_sidebar_counts();
        self.set_status(if ignore {
            tr("Đã bỏ qua các mục đã chọn.", "Selected files ignored.")
        } else {
            tr("Đã đưa các tệp trở lại danh sách.", "Files returned to the review list.")
        });
    }

    pub fn confirm_trash(&self) {
        let items = self.selected_items();
        debug(&format!("confirm_trash: {} selected", items.len()));
        if items.is_empty() {
            return;
        }
        let total: i64 = items.iter().map(|c| c.size).sum();
        let alert = NSAlert::new(self.mtm);
        alert.setAlertStyle(NSAlertStyle::Warning);
        alert.setMessageText(&ns(&tr(
            &format!("Chuyển {} tệp vào Thùng rác?", items.len()),
            &format!("Move {} files to Trash?", items.len()),
        )));
        alert.setInformativeText(&ns(&tr(
            &format!("{} · Dung lượng chỉ được giải phóng sau khi dọn sạch Thùng rác.", format::bytes(total)),
            &format!("{} · Space is reclaimed after the Trash is emptied.", format::bytes(total)),
        )));
        let ok = alert.addButtonWithTitle(&ns(&tr("Chuyển vào Thùng rác", "Move to Trash")));
        ok.setHasDestructiveAction(true);
        alert.addButtonWithTitle(&ns(&tr("Huỷ", "Cancel")));
        let answer = alert.runModal();
        debug(&format!("trash alert answer: {answer}"));
        if answer == NSAlertFirstButtonReturn {
            self.clean(items, false);
        }
    }

    pub fn confirm_delete(&self) {
        let items = self.selected_items();
        if items.is_empty() {
            return;
        }
        let phrase = format!("DELETE {}", items.len());
        let total: i64 = items.iter().map(|c| c.size).sum();
        let mut preview: Vec<String> = items.iter().take(5).map(|c| c.path.replace(&*self.home.to_string_lossy(), "~")).collect();
        if items.len() > 5 {
            preview.push(format!("+ {}", items.len() - 5));
        }
        let alert = NSAlert::new(self.mtm);
        alert.setAlertStyle(NSAlertStyle::Critical);
        alert.setMessageText(&ns(&tr(
            &format!("Xoá vĩnh viễn {} tệp ({})?", items.len(), format::bytes(total)),
            &format!("Permanently delete {} files ({})?", items.len(), format::bytes(total)),
        )));
        alert.setInformativeText(&ns(&format!(
            "{}\n\n{}\n\n{}",
            tr(
                "Không thể hoàn tác, không qua Thùng rác. Chỉ tệp còn khớp với lần quét mới bị xoá.",
                "Cannot be undone and skips the Trash. Only files still matching the scan are deleted."
            ),
            preview.join("\n"),
            tr(&format!("Gõ {phrase} để xác nhận:"), &format!("Type {phrase} to confirm:")),
        )));
        let del = alert.addButtonWithTitle(&ns(&tr("Xoá vĩnh viễn", "Delete permanently")));
        del.setHasDestructiveAction(true);
        del.setEnabled(false);
        alert.addButtonWithTitle(&ns(&tr("Huỷ", "Cancel")));
        let field = NSTextField::textFieldWithString(&ns(""), self.mtm);
        field.setPlaceholderString(Some(&ns(&phrase)));
        field.setFont(Some(&mono(13.0)));
        field.setFrame(NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(260.0, 24.0)));
        let del_ref = del.clone();
        let want = phrase.clone();
        let watcher = Target::new(self.mtm, move |obj| {
            let typed = obj
                .and_then(|o| o.downcast_ref::<NSTextField>())
                .map(|f| f.stringValue().to_string())
                .unwrap_or_default();
            del_ref.setEnabled(typed == want);
        });
        unsafe { field.setDelegate(Some(objc2::runtime::ProtocolObject::from_ref(&*watcher))) };
        alert.setAccessoryView(Some(&field));
        alert.window().setInitialFirstResponder(Some(&field));
        let answer = alert.runModal();
        unsafe { field.setDelegate(None) };
        drop(watcher);
        if answer == NSAlertFirstButtonReturn && field.stringValue().to_string() == phrase {
            self.clean(items, true);
        }
    }

    fn clean(&self, items: Vec<Candidate>, permanent: bool) {
        let s = self.settings.borrow().clone();
        let now = chrono::Local::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, false);
        let mut done = vec![];
        let mut failed = vec![];
        for c in &items {
            let r = if permanent {
                delete_permanently(c, &self.home, &s).map_err(|e| e.to_string())
            } else {
                validate_for_action(c, &self.home, &s)
                    .map_err(|e| e.to_string())
                    .and_then(|_| system::move_to_trash(&c.path))
            };
            let action = if permanent { "delete" } else { "trash" };
            let result = r.as_ref().map(|_| "ok".to_string()).unwrap_or_else(|e| e.clone());
            let _ = self.store.record_action(&now, action, &c.path, &result);
            match r {
                Ok(()) => done.push(c.id.clone()),
                Err(e) => failed.push(format!("{}: {e}", Path::new(&c.path).file_name().unwrap_or_default().to_string_lossy())),
            }
        }
        if let Err(e) = self.store.remove_candidates(&done).and_then(|_| self.store.remove_bookmarks_for_files(&done)) {
            failed.push(e);
        }
        self.candidates.borrow_mut().retain(|c| !done.contains(&c.id));
        self.list_items.borrow_mut().retain(|c| !done.contains(&c.id));
        self.reload_lists();
        self.selected.borrow_mut().clear();
        self.refresh_volumes();
        self.update_sidebar_counts();
        let mut msg = if permanent {
            tr(&format!("Đã xoá vĩnh viễn {} tệp.", done.len()), &format!("Deleted {} files.", done.len()))
        } else {
            tr(&format!("Đã chuyển {} tệp vào Thùng rác.", done.len()), &format!("Moved {} files to Trash.", done.len()))
        };
        if !failed.is_empty() {
            msg.push_str(&format!(" {} {}", tr("Lỗi:", "Failed:"), failed.join("; ")));
        }
        self.set_status(msg);
    }

    // ------------------------------------------------------------ overview cards

    pub fn cards(&self) -> Vec<CardPref> {
        normalize(&self.settings.borrow().overview_cards)
    }

    fn save_cards(&self, cards: Vec<CardPref>) {
        let mut s = self.settings.borrow().clone();
        s.overview_cards = cards;
        if let Err(e) = self.store.save_settings(&s) {
            return self.set_status(e);
        }
        *self.settings.borrow_mut() = s;
        self.request_render();
    }

    pub fn set_card_size(&self, id: &str, size: CardSize) {
        let cards = self.cards().into_iter().map(|c| if c.id == id { CardPref { id: c.id, size } } else { c }).collect();
        self.save_cards(cards);
    }

    pub fn move_card(&self, id: &str, step: isize) {
        let mut cards = self.cards();
        let Some(i) = cards.iter().position(|c| c.id == id) else { return };
        let j = i as isize + step;
        if j < 0 || j as usize >= cards.len() {
            return;
        }
        cards.swap(i, j as usize);
        self.save_cards(cards);
    }

    pub fn reset_cards(&self) {
        self.save_cards(sdc_core::overview::default_cards());
    }

    /// Live snapshot of CPU/memory/temp values.
    pub fn live_metrics(&self) -> (Vec<f64>, usize, Option<MemInfo>, Option<TempSizes>) {
        match self.live.lock() {
            Ok(l) => (l.cpu.iter().copied().collect(), l.cores, l.mem, l.temp),
            Err(_) => (vec![], 0, None, None),
        }
    }

    /// Short value for a metric: "147 GB", "6.8 GB", "23%", "11.6 GB".
    pub fn metric_text(&self, id: &str) -> String {
        let (cpu, _, mem, temp) = self.live_metrics();
        match id {
            "disk" => self.volumes.borrow().first().map(|v| format::bytes(v.free_bytes)).unwrap_or_else(|| "—".into()),
            "temp" => temp.map(|t| format::bytes(t.total())).unwrap_or_else(|| "…".into()),
            "cpu" => cpu.last().map(|v| format!("{v:.0}%")).unwrap_or_else(|| "—".into()),
            "ram" => mem.map(|m| gib(m.used())).unwrap_or_else(|| "—".into()),
            _ => String::new(),
        }
    }

    /// Menu bar item: status dot plus the chosen values, and live tray menu lines.
    pub fn update_tray_title(&self) {
        let Some(item) = self.status_item.borrow().clone() else { return };
        let Some(button) = item.button(self.mtm) else { return };
        let s = self.settings.borrow().clone();
        let items: Vec<&str> =
            sdc_core::overview::MENU_BAR_ITEMS.iter().copied().filter(|id| s.menu_bar_items.iter().any(|x| x == id)).collect();
        let dot = s.menu_bar_dot || items.is_empty();
        button.setImage(if dot { Some(system::tray_image(self.worst())) } else { None }.as_deref());
        button.setImagePosition(if items.is_empty() { NSCellImagePosition::ImageOnly } else { NSCellImagePosition::ImageLeading });
        let font = digits(13.0, W::Regular);
        if s.menu_bar_style == "icons" && !items.is_empty() {
            button.setAttributedTitle(&system::icon_title(&items.iter().map(|id| (metric_symbol(id), self.metric_text(id))).collect::<Vec<_>>(), &font));
        } else {
            let text = match s.menu_bar_style.as_str() {
                "labels" => items.iter().map(|id| format!("{} {}", metric_short_label(id), self.metric_text(id))).collect::<Vec<_>>().join("  "),
                _ => items.iter().map(|id| self.metric_text(id)).collect::<Vec<_>>().join(" · "),
            };
            button.setFont(Some(&font));
            button.setTitle(&ns(&text));
        }
        for (id, mi) in self.tray_lines.borrow().iter() {
            mi.setTitle(&ns(&self.tray_line(id)));
        }
    }

    /// One info line of the tray menu.
    pub fn tray_line(&self, id: &str) -> String {
        let (_, cores, mem, temp) = self.live_metrics();
        match id {
            "temp" => format!(
                "{} · {}",
                tr("Tệp tạm", "Temporary files"),
                temp.map(|t| format::bytes(t.total())).unwrap_or_else(|| tr("đang đo…", "measuring…"))
            ),
            "cpu" => format!("CPU · {} · {} {}", self.metric_text("cpu"), cores, tr("nhân", "cores")),
            "ram" => format!(
                "{} · {}",
                tr("Bộ nhớ", "Memory"),
                mem.map(|m| format!("{} / {}", gib(m.used()), gib(m.total))).unwrap_or_else(|| "—".into())
            ),
            _ => String::new(),
        }
    }

    /// Refresh the CPU and memory cards in place (no re-render).
    pub fn update_live_cards(&self) {
        let refs = self.refs.borrow();
        if refs.cpu_value.is_none() && refs.mem_value.is_none() {
            return;
        }
        let (cpu, _, mem, _) = self.live_metrics();
        if let Some((l, size)) = refs.cpu_value.as_ref() {
            let now = cpu.last().copied().unwrap_or(0.0);
            set_hero_total(l, &format!("{now:.0}"), "%", "", *size);
        }
        if let Some(sp) = refs.cpu_spark.as_ref() {
            sp.set_values(cpu.clone());
        }
        if refs.cpu_stats.len() == 2 && !cpu.is_empty() {
            let avg = cpu.iter().sum::<f64>() / cpu.len() as f64;
            let peak = cpu.iter().copied().fold(0.0, f64::max);
            refs.cpu_stats[0].setStringValue(&ns(&format!("{avg:.0}%")));
            refs.cpu_stats[1].setStringValue(&ns(&format!("{peak:.0}%")));
        }
        if let Some(m) = mem {
            if let Some((l, size)) = refs.mem_value.as_ref() {
                let used = gib(m.used());
                let (n, u) = used.rsplit_once(' ').unwrap_or((&used, ""));
                set_hero_total(l, n, u, &format!("/ {}", gib(m.total)), *size);
            }
            let t = m.total.max(1) as f64;
            let (a, w, c) = (m.app as f64 / t, m.wired as f64 / t, m.compressed as f64 / t);
            if let Some(b) = refs.mem_bar.as_ref() {
                b.set_segments(mem_segments(a, w, c));
            }
            for (part, l, named) in &refs.mem_labels {
                let (name, bytes) = match part {
                    0 => (tr("Ứng dụng", "App"), m.app),
                    1 => (tr("Cố định", "Wired"), m.wired),
                    2 => (tr("Nén", "Compressed"), m.compressed),
                    _ => (tr("Còn trống", "Available"), m.total.saturating_sub(m.used())),
                };
                l.setStringValue(&ns(&if *named { format!("{name} {}", gib(bytes)) } else { gib(bytes) }));
            }
        }
    }

    // ------------------------------------------------------------ folder usage

    /// No measurement yet, or the last one is older than 6 hours.
    fn usage_stale(&self) -> bool {
        let at = self.usage_home.borrow().as_ref().map(|u| u.at.clone());
        match at.and_then(|t| chrono::DateTime::parse_from_rfc3339(&t).ok()) {
            Some(t) => chrono::Utc::now() - t.with_timezone(&chrono::Utc) > chrono::Duration::hours(6),
            None => true,
        }
    }

    /// Measure folder sizes in the background (home first, then the disk split).
    pub fn measure_usage(&self) {
        if self.usage_measuring.replace(true) {
            return;
        }
        self.usage_cancel.store(false, Ordering::Relaxed);
        let home = self.home.clone();
        let store = self.store.clone();
        let progress = self.usage_progress.clone();
        let cancel = self.usage_cancel.clone();
        let vol = self.volumes.borrow().first().map(|v| (v.total_bytes, v.free_bytes));
        let include_cloud = self.settings.borrow().include_icloud;
        let blocked = self.settings.borrow().blocked_private(&self.home);
        std::thread::spawn(move || {
            let at = chrono::Local::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, false);
            let report = |p: &str| {
                if let Ok(mut g) = progress.lock() {
                    *g = p.to_string();
                }
            };
            let stop = || cancel.load(Ordering::Relaxed);
            let home_u = sdc_core::usage::home_usage(&home, include_cloud, &blocked, &at, &report, &stop);
            let disk_u = match (stop(), vol) {
                (false, Some((total, free))) => {
                    Some(sdc_core::usage::disk_usage(&home, home_u.total(), total, free, &at, &report, &stop))
                }
                _ => None,
            };
            let cancelled = stop();
            if !cancelled {
                let _ = store.save_usage("home", &home_u);
                if let Some(d) = &disk_u {
                    let _ = store.save_usage("disk", d);
                }
            }
            later(move |a| {
                a.usage_measuring.set(false);
                if !cancelled {
                    *a.usage_home.borrow_mut() = Some(home_u);
                    if disk_u.is_some() {
                        *a.usage_disk.borrow_mut() = disk_u;
                    }
                }
                if a.page.get() == Page::Storage {
                    a.request_render();
                }
            });
        });
    }

    pub fn stop_usage(&self) {
        self.usage_cancel.store(true, Ordering::Relaxed);
    }

    // ------------------------------------------------------------ bookmark lists

    /// Reload lists and the bookmark index from the store and refresh the sidebar.
    pub fn reload_lists(&self) {
        if let Ok(l) = self.store.lists() {
            *self.lists.borrow_mut() = l;
        }
        if let Ok(i) = self.store.bookmark_index() {
            *self.bookmark_index.borrow_mut() = i;
        }
        crate::pages::rebuild_sidebar_lists(self);
        self.update_sidebar_counts();
    }

    fn list_name(&self, id: i64) -> String {
        self.lists.borrow().iter().find(|l| l.id == id).map(|l| l.name.clone()).unwrap_or_default()
    }

    /// Modal prompt for a list name.
    fn ask_name(&self, title: &str, initial: &str, ok: &str) -> Option<String> {
        let alert = NSAlert::new(self.mtm);
        alert.setMessageText(&ns(title));
        alert.addButtonWithTitle(&ns(ok));
        alert.addButtonWithTitle(&ns(&tr("Huỷ", "Cancel")));
        let field = NSTextField::textFieldWithString(&ns(initial), self.mtm);
        field.setPlaceholderString(Some(&ns(&tr("Tên danh sách", "List name"))));
        field.setFrame(NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(260.0, 24.0)));
        alert.setAccessoryView(Some(&field));
        alert.window().setInitialFirstResponder(Some(&field));
        if alert.runModal() != NSAlertFirstButtonReturn {
            return None;
        }
        let name = field.stringValue().to_string().trim().to_string();
        (!name.is_empty()).then_some(name)
    }

    /// Ask for a name, create the list, and optionally put the selection in it.
    pub fn new_list(&self, add_selection: bool) {
        let Some(name) = self.ask_name(&tr("Danh sách mới", "New list"), "", &tr("Tạo", "Create")) else { return };
        match self.store.create_list(&name) {
            Err(e) => self.set_status(e),
            Ok(id) => {
                self.reload_lists();
                if add_selection {
                    self.add_selected_to_list(id);
                } else {
                    self.navigate(Page::List(id));
                }
            }
        }
    }

    pub fn add_selected_to_list(&self, id: i64) {
        let items = self.selected_items();
        if items.is_empty() {
            return;
        }
        let now = chrono::Local::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, false);
        if let Err(e) = self.store.add_bookmarks(id, &items, &now) {
            return self.set_status(e);
        }
        self.reload_lists();
        let name = self.list_name(id);
        self.set_status(tr(
            &format!("Đã thêm {} tệp vào \"{name}\".", items.len()),
            &format!("Added {} files to \"{name}\".", items.len()),
        ));
    }

    pub fn remove_selected_from_list(&self) {
        let Page::List(id) = self.page.get() else { return };
        let ids: Vec<String> = self.selected_items().into_iter().map(|c| c.id).collect();
        if ids.is_empty() {
            return;
        }
        if let Err(e) = self.store.remove_bookmarks(id, &ids) {
            return self.set_status(e);
        }
        self.selected.borrow_mut().clear();
        self.reload_lists();
        self.set_status(tr(&format!("Đã bỏ {} tệp khỏi danh sách.", ids.len()), &format!("Removed {} files from the list.", ids.len())));
    }

    pub fn rename_list(&self, id: i64) {
        let old = self.list_name(id);
        let Some(name) = self.ask_name(&tr("Đổi tên danh sách", "Rename list"), &old, &tr("Đổi tên", "Rename")) else { return };
        if let Err(e) = self.store.rename_list(id, &name) {
            return self.set_status(e);
        }
        self.reload_lists();
        self.highlight_nav();
        self.request_render();
    }

    pub fn delete_list(&self, id: i64) {
        let name = self.list_name(id);
        let alert = NSAlert::new(self.mtm);
        alert.setAlertStyle(NSAlertStyle::Warning);
        alert.setMessageText(&ns(&tr(&format!("Xoá danh sách \"{name}\"?"), &format!("Delete the list \"{name}\"?"))));
        alert.setInformativeText(&ns(&tr("Chỉ xoá danh sách, các tệp vẫn còn nguyên.", "Only the list is removed; the files are not touched.")));
        let ok = alert.addButtonWithTitle(&ns(&tr("Xoá danh sách", "Delete list")));
        ok.setHasDestructiveAction(true);
        alert.addButtonWithTitle(&ns(&tr("Huỷ", "Cancel")));
        if alert.runModal() != NSAlertFirstButtonReturn {
            return;
        }
        if let Err(e) = self.store.delete_list(id) {
            return self.set_status(e);
        }
        self.reload_lists();
        if self.page.get() == Page::List(id) {
            self.navigate(Page::Files);
        }
    }

    /// Drop scan results and bookmarks whose file no longer exists (checked off the main thread).
    pub fn prune_missing(&self) {
        if self.pruning.replace(true) {
            return;
        }
        let cands = self.candidates.borrow().clone();
        let store = self.store.clone();
        std::thread::spawn(move || {
            let marks = store.all_bookmarks().unwrap_or_default();
            let gone_c: Vec<String> = cands.iter().filter(|c| !still_exists(c)).map(|c| c.id.clone()).collect();
            let gone_b: Vec<String> = marks.iter().filter(|c| !still_exists(c)).map(|c| c.id.clone()).collect();
            let err = store.remove_candidates(&gone_c).and_then(|_| store.remove_bookmarks_for_files(&gone_b)).err();
            later(move |a| {
                a.pruning.set(false);
                if let Some(e) = err {
                    return a.set_status(e);
                }
                if gone_c.is_empty() && gone_b.is_empty() {
                    return;
                }
                debug(&format!("pruned {} results, {} bookmarks", gone_c.len(), gone_b.len()));
                a.candidates.borrow_mut().retain(|c| !gone_c.contains(&c.id));
                a.list_items.borrow_mut().retain(|c| !gone_b.contains(&c.id));
                a.selected.borrow_mut().retain(|id| !gone_c.contains(id) && !gone_b.contains(id));
                a.reload_lists();
                a.request_render();
            });
        });
    }

    // ------------------------------------------------------------ settings

    pub fn finish_setup(&self) {
        {
            let mut s = self.settings.borrow_mut();
            s.scan_home = self.setup_home.get();
            s.setup_done = true;
            if let Err(e) = self.store.save_settings(&s) {
                s.setup_done = false;
                drop(s);
                return self.set_status(e);
            }
        }
        if self.settings.borrow().open_at_login {
            if let Err(e) = system::set_open_at_login(true) {
                *self.status.borrow_mut() = e;
            }
        }
        crate::pages::show_sidebar(self, true);
        self.refresh_volumes();
        self.navigate(Page::Overview);
        self.start_scan();
    }

    pub fn draft(&self) -> Settings {
        self.draft.borrow_mut().get_or_insert_with(|| self.settings.borrow().clone()).clone()
    }

    pub fn edit_draft(&self, f: impl FnOnce(&mut Settings)) {
        let mut d = self.draft();
        f(&mut d);
        *self.draft.borrow_mut() = Some(d);
    }

    /// Keep typed (possibly invalid) field text across settings re-renders.
    pub fn capture_settings_text(&self) {
        let refs = self.refs.borrow();
        if refs.fields.len() == 4 {
            let mut t: [String; 5] = Default::default();
            for (i, f) in refs.fields.iter().enumerate() {
                t[i] = f.stringValue().to_string();
            }
            t[4] = refs.exclusions.as_ref().map(|v| v.string().to_string()).unwrap_or_default();
            *self.draft_text.borrow_mut() = Some(t);
        }
    }

    pub fn pick_root(&self) {
        self.capture_settings_text();
        let panel = NSOpenPanel::openPanel(self.mtm);
        panel.setCanChooseDirectories(true);
        panel.setCanChooseFiles(false);
        panel.setAllowsMultipleSelection(false);
        panel.setMessage(Some(&ns(&tr("Chọn thư mục quét", "Choose a folder to scan"))));
        if panel.runModal() != NSModalResponseOK {
            return;
        }
        let Some(url) = panel.URLs().firstObject() else { return };
        let Some(path) = url.path() else { return };
        let path = clean(Path::new(&path.to_string())).to_string_lossy().into_owned();
        let d = self.draft();
        if protected(Path::new(&path), &self.home, &d.exclusions) {
            return self.set_status(tr("Thư mục này được bảo vệ.", "This folder is protected."));
        }
        // Picking a folder inside Desktop (etc.) is consent to read that folder.
        if let Some(f) = Settings::private_folder_of(Path::new(&path), &self.home) {
            if !d.private_folders.iter().any(|p| p == f) {
                self.edit_draft(|d| d.private_folders.push(f.to_string()));
            }
        }
        if !d.roots.contains(&path) {
            self.edit_draft(|d| d.roots.push(path));
        }
        self.request_render();
    }

    pub fn remove_root(&self, root: &str) {
        self.capture_settings_text();
        let root = root.to_string();
        self.edit_draft(|d| d.roots.retain(|r| *r != root));
        self.request_render();
    }

    pub fn save_settings(&self) {
        self.capture_settings_text();
        let text = self.draft_text.borrow().clone().unwrap_or_default();
        let parse = |s: &str| s.trim().parse::<i64>().ok().filter(|v| *v > 0);
        let (Some(size), Some(age), Some(yellow), Some(red)) = (parse(&text[0]), parse(&text[1]), parse(&text[2]), parse(&text[3]))
        else {
            return self.set_status(tr("Ngưỡng phải là số nguyên dương.", "Thresholds must be positive integers."));
        };
        let mut s = self.draft();
        s.min_size_bytes = size * MB;
        s.min_age_days = age;
        s.yellow_gb = yellow;
        s.red_gb = red;
        s.exclusions.clear();
        for line in text[4].lines().map(str::trim).filter(|l| !l.is_empty()) {
            if !Path::new(line).is_absolute() {
                return self.set_status(tr(
                    "Đường dẫn loại trừ phải là đường dẫn tuyệt đối.",
                    "Exclusions must be absolute paths.",
                ));
            }
            s.exclusions.push(clean(Path::new(line)).to_string_lossy().into_owned());
        }
        if let Err(e) = s.validate().and_then(|_| self.store.save_settings(&s)) {
            return self.set_status(e);
        }
        let login = system::set_open_at_login(s.open_at_login);
        *self.settings.borrow_mut() = s.clone();
        *self.draft.borrow_mut() = None;
        *self.draft_text.borrow_mut() = None;
        self.refresh_volumes();
        self.update_footer();
        self.set_status(match login {
            Ok(()) => tr("Đã lưu cài đặt.", "Settings saved."),
            Err(e) => e,
        });
        if s.auto_scan {
            self.maybe_scan();
        }
    }

    pub fn revert_settings(&self) {
        *self.draft.borrow_mut() = None;
        *self.draft_text.borrow_mut() = None;
        self.set_status("");
    }

    // ------------------------------------------------------------ live stats

    pub fn start_sampler(&self) {
        let live = self.live.clone();
        std::thread::spawn(move || {
            let machine = sysstats::machine_name();
            if let Ok(mut l) = live.lock() {
                l.machine = machine;
            }
        });
        // Temporary folder sizes: a walk, so on its own thread every 10 minutes.
        let live = self.live.clone();
        let home = self.home.clone();
        std::thread::spawn(move || loop {
            let t = sysstats::temp_sizes(&home);
            if let Ok(mut l) = live.lock() {
                l.temp = Some(t);
                l.temp_rev += 1;
            }
            std::thread::sleep(Duration::from_secs(600));
        });
        let live = self.live.clone();
        std::thread::spawn(move || {
            let mut procs = ProcSampler::default();
            let mut cpu = CpuSampler::default();
            let cores = sysstats::cpu_cores();
            let mut tick: u64 = 0;
            loop {
                let load = cpu.sample();
                let mem = if tick.is_multiple_of(2) { sysstats::memory() } else { None };
                if let Ok(mut l) = live.lock() {
                    l.cores = cores;
                    if let Some(v) = load {
                        l.cpu.push_back(v);
                        while l.cpu.len() > CPU_HISTORY {
                            l.cpu.pop_front();
                        }
                    }
                    if mem.is_some() {
                        l.mem = mem;
                    }
                }
                let (r, w) = sysstats::disk_io_totals();
                let p = if tick.is_multiple_of(2) { Some(procs.sample()) } else { None };
                let extra = if tick.is_multiple_of(10) { Some((sysstats::disk_images(), sysstats::mounts())) } else { None };
                if let Ok(mut l) = live.lock() {
                    if let Some(h) = l.history.as_mut() {
                        h.push(r, w, Instant::now());
                    }
                    if let Some(p) = p {
                        l.procs = p;
                    }
                    if let Some((i, m)) = extra {
                        l.images = i;
                        l.mounts = m;
                    }
                }
                tick += 1;
                std::thread::sleep(Duration::from_secs(1));
            }
        });
    }

    pub fn range_secs(&self) -> usize {
        [60, 300, 900, 3600][self.range.get().clamp(0, 3) as usize]
    }

    /// 1 Hz: refresh live labels and charts on screen.
    pub fn tick(&self) {
        if self.usage_measuring.get() {
            if let Some(l) = self.refs.borrow().usage_status.as_ref() {
                let p = self.usage_progress.lock().map(|p| p.clone()).unwrap_or_default();
                l.setStringValue(&ns(&tr(&format!("Đang đo {p}…"), &format!("Measuring {p}…"))));
            }
        }
        let n = self.ticks.get() + 1;
        self.ticks.set(n);
        self.update_live_cards();
        if n.is_multiple_of(2) {
            self.update_tray_title();
        }
        let temp_rev = self.live.lock().map(|l| l.temp_rev).unwrap_or(0);
        if temp_rev != self.temp_seen.get() {
            self.temp_seen.set(temp_rev);
            if self.page.get() == Page::Overview {
                self.request_render();
            }
        }
        let (latest, totals, buckets, procs) = {
            let Ok(l) = self.live.lock() else { return };
            let Some(h) = l.history.as_ref() else { return };
            let cols = self.refs.borrow().chart.as_ref().map(|c| c.columns()).unwrap_or(0);
            let buckets = if cols > 0 { Some(h.buckets(self.range_secs(), cols)) } else { None };
            (h.latest(), (h.total_read, h.total_written), buckets, l.procs.clone())
        };
        for item in self.nav.borrow().iter() {
            if item.page == Page::Disk {
                item.metric.setStringValue(&ns(&format::rate(latest.read + latest.write)));
            }
        }
        if let Some(f) = self.footer_machine.borrow().as_ref() {
            if let Ok(l) = self.live.lock() {
                if !l.machine.is_empty() && f.stringValue().to_string() != l.machine {
                    f.setStringValue(&ns(&l.machine));
                }
            }
        }
        if self.paused.get() {
            return;
        }
        let refs = self.refs.borrow();
        if let Some(l) = refs.rate_total.as_ref() {
            let (n, u) = format::rate_parts(latest.read + latest.write);
            set_hero(l, &n, &u, if self.page.get() == Page::Disk { 44.0 } else { 40.0 });
        }
        if let Some(l) = refs.rate_read.as_ref() {
            l.setStringValue(&ns(&format::rate(latest.read)));
        }
        if let Some(l) = refs.rate_write.as_ref() {
            l.setStringValue(&ns(&format::rate(latest.write)));
        }
        if let Some(l) = refs.read_total.as_ref() {
            l.setStringValue(&ns(&format::bytes(totals.0 as i64)));
        }
        if let Some(l) = refs.written_total.as_ref() {
            l.setStringValue(&ns(&format::bytes(totals.1 as i64)));
        }
        if let (Some(chart), Some(b)) = (refs.chart.as_ref(), buckets) {
            chart.set_samples(b);
        }
        if let Some(stack) = refs.procs.as_ref() {
            crate::pages::fill_procs(self, stack, &procs);
        }
    }

    pub fn update_sidebar_counts(&self) {
        let active = self.active().len();
        let ignored = self.candidates.borrow().iter().filter(|c| c.ignored).count();
        let free = self.volumes.borrow().first().map(|v| format::bytes(v.free_bytes)).unwrap_or_default();
        for item in self.nav.borrow().iter() {
            match item.page {
                Page::Files => item.metric.setStringValue(&ns(&active.to_string())),
                Page::Ignored => item.metric.setStringValue(&ns(&if ignored > 0 { ignored.to_string() } else { String::new() })),
                Page::Volumes => item.metric.setStringValue(&ns(&free)),
                Page::List(id) => {
                    let n = self.lists.borrow().iter().find(|l| l.id == id).map(|l| l.count).unwrap_or(0);
                    item.metric.setStringValue(&ns(&if n > 0 { n.to_string() } else { String::new() }));
                }
                _ => {}
            }
        }
        // Volume metric color follows the disk level.
        let level_color = level_color(self.worst());
        let page = self.page.get();
        for item in self.nav.borrow_mut().iter_mut() {
            if item.page == Page::Volumes {
                item.metric_color = level_color.clone();
                if page != Page::Volumes {
                    item.metric.setTextColor(Some(&level_color));
                }
            }
        }
        self.update_tray();
    }

    pub fn update_footer(&self) {
        if let Some(l) = self.footer_status.borrow().as_ref() {
            let (on, every) = {
                let s = self.settings.borrow();
                (s.auto_scan, interval_text(s.scan_interval_minutes))
            };
            l.setStringValue(&ns(&if on {
                tr(&format!("Tự quét mỗi {every} · bật"), &format!("Auto-scan every {every} · on"))
            } else {
                tr("Tự quét · tắt", "Auto-scan · off")
            }));
        }
    }
}

/// Trace to stderr when SDC_DEBUG is set.
pub fn debug(msg: &str) {
    if std::env::var_os("SDC_DEBUG").is_some() {
        eprintln!("[sdc] {msg}");
    }
}

pub fn level_color(l: Level) -> Retained<NSColor> {
    match l {
        Level::Green => Palette::green(),
        Level::Yellow => Palette::yellow(),
        Level::Red => Palette::red(),
    }
}

pub fn level_text(l: Level) -> String {
    match l {
        Level::Green => tr("Dư dả", "Healthy"),
        Level::Yellow => tr("Sắp đầy", "Low"),
        Level::Red => tr("Gần hết", "Critical"),
    }
}

/// "10 min", "2 h" style interval label.
pub fn interval_text(minutes: i64) -> String {
    if minutes >= 60 && minutes % 60 == 0 {
        tr(&format!("{} giờ", minutes / 60), &format!("{} h", minutes / 60))
    } else {
        tr(&format!("{minutes} phút"), &format!("{minutes} min"))
    }
}

/// Binary gigabytes, like Activity Monitor: "11.6 GB".
pub fn gib(bytes: u64) -> String {
    format!("{:.1} GB", bytes as f64 / 1_073_741_824.0)
}

pub fn mem_segments(app: f64, wired: f64, compressed: f64) -> Vec<(f64, f64, Retained<NSColor>)> {
    vec![
        (0.0, app, Palette::blue()),
        (app, app + wired, Palette::indigo()),
        (app + wired, app + wired + compressed, Palette::tint(&Palette::blue(), 0.45)),
    ]
}

pub fn metric_symbol(id: &str) -> &'static str {
    match id {
        "disk" => "internaldrive",
        "temp" => "clock.arrow.circlepath",
        "cpu" => "cpu",
        _ => "memorychip",
    }
}

pub fn metric_short_label(id: &str) -> String {
    match id {
        "disk" => tr("Ổ", "Disk"),
        "temp" => tr("Tạm", "Temp"),
        "cpu" => "CPU".into(),
        _ => "RAM".into(),
    }
}
