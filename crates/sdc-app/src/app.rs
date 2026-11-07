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
use sdc_core::scan::{scan, volume_stats};
use sdc_core::store::Store;
use sdc_core::sysstats::{self, DiskImage, Mount, ProcIo, ProcSampler, RateHistory};
use std::cell::{Cell, OnceCell, RefCell};
use std::collections::HashSet;
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
}

#[derive(Default)]
pub struct Live {
    pub history: Option<RateHistory>,
    pub procs: Vec<ProcIo>,
    pub images: Vec<DiskImage>,
    pub mounts: Vec<Mount>,
    pub machine: String,
}

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
    pub cancel: Arc<AtomicBool>,
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
            cancel: Arc::new(AtomicBool::new(false)),
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
        for item in self.nav.borrow().iter() {
            let on = item.page == page;
            item.row.set_selected(on);
            item.title.setTextColor(Some(&*if on { NSColor::whiteColor() } else { Palette::primary() }));
            item.metric.setTextColor(Some(&*if on { NSColor::whiteColor() } else { item.metric_color.clone() }));
        }
        self.request_render();
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

    pub fn selected_items(&self) -> Vec<Candidate> {
        let sel = self.selected.borrow();
        self.candidates.borrow().iter().filter(|c| sel.contains(&c.id)).cloned().collect()
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
        if let Some(button) = item.button(self.mtm) {
            button.setImage(Some(&system::tray_image(self.worst())));
        }
        crate::pages::fill_tray_menu(self, &item);
    }

    // ------------------------------------------------------------ scanning

    pub fn maybe_scan(&self) {
        let s = self.settings.borrow().clone();
        let due = is_zero_time(&s.last_scan) || chrono::Utc::now() - s.last_scan >= chrono::Duration::hours(2);
        if s.setup_done && s.auto_scan && !self.scanning.get() && due {
            self.start_scan();
        }
    }

    pub fn start_scan(&self) {
        if self.scanning.get() || !self.settings.borrow().setup_done {
            return;
        }
        self.scanning.set(true);
        self.cancel.store(false, Ordering::Relaxed);
        self.set_status(tr("Đang quét…", "Scanning…"));
        let settings = self.settings.borrow().clone();
        let before = self.candidates.borrow().clone();
        let before_level = Level::parse(&settings.last_alert_level);
        let home = self.home.clone();
        let store = self.store.clone();
        let cancel = self.cancel.clone();
        std::thread::spawn(move || {
            let mut ai_error = None;
            let result = (|| {
                let r = scan(&home, &settings, None, &cancel).map_err(|e| e.to_string())?;
                let mut cands = r.candidates;
                if settings.ai_review {
                    match review::find_claude() {
                        Some(bin) => {
                            let (reviewed, err) = review::review(&cands, &home, &settings, Some(&bin), None);
                            cands = reviewed;
                            ai_error = err;
                        }
                        None => ai_error = Some("claude CLI not found".into()),
                    }
                }
                store.replace_candidates(&cands)?;
                let saved = store.load_candidates()?;
                let mut s = settings.clone();
                s.last_scan = chrono::Utc::now();
                s.last_alert_ids = candidate_ids(&saved);
                s.last_alert_level = worst_level(&r.volumes).as_str().to_string();
                store.save_settings(&s)?;
                Ok((saved, r.volumes, r.skipped, s))
            })();
            let outcome = ScanOutcome { result, ai_error, before, before_level };
            later(move |a| a.finish_scan(outcome));
        });
    }

    fn finish_scan(&self, o: ScanOutcome) {
        self.scanning.set(false);
        match o.result {
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
        let items: Vec<Candidate> = self.candidates.borrow().iter().filter(|c| sel.contains(&c.id)).cloned().collect();
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
        if let Err(e) = self.store.remove_candidates(&done) {
            failed.push(e);
        }
        self.candidates.borrow_mut().retain(|c| !done.contains(&c.id));
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
        let live = self.live.clone();
        std::thread::spawn(move || {
            let mut procs = ProcSampler::default();
            let mut tick: u64 = 0;
            loop {
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
            let on = self.settings.borrow().auto_scan;
            l.setStringValue(&ns(&if on {
                tr("Tự quét mỗi 2 giờ · bật", "Auto-scan every 2h · on")
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
