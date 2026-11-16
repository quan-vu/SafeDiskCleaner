//! Scan progress sheet: spinner, current folder, live counts, a scrolling log,
//! and a hint when the scan is stuck (usually a macOS privacy prompt).

use crate::app::{app, tr, App};
use crate::ui::*;
use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{AllocAnyThread, MainThreadOnly};
use objc2_app_kit::*;
use objc2_foundation::*;
use std::ptr::NonNull;
use std::sync::atomic::Ordering;
use std::time::Duration;

/// A stall longer than this while reading folders shows the permission hint.
const STALL_HINT: Duration = Duration::from_secs(4);

pub struct ScanSheet {
    panel: Retained<NSPanel>,
    spinner: Retained<NSProgressIndicator>,
    done_icon: Retained<NSImageView>,
    phase: Retained<NSTextField>,
    current: Retained<NSTextField>,
    counts: Retained<NSTextField>,
    hint_box: Retained<NSStackView>,
    hint: Retained<NSTextField>,
    log: Retained<NSTextView>,
    stop: Retained<NSButton>,
    close: Retained<NSButton>,
    timer: Option<Retained<NSTimer>>,
    _targets: Vec<Retained<Target>>,
    /// Scan generation whose log the view currently shows.
    shown_gen: u64,
}

fn build(a: &App) -> ScanSheet {
    let mtm = a.mtm;
    let rect = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(640.0, 500.0));
    let panel = unsafe {
        NSPanel::initWithContentRect_styleMask_backing_defer(
            NSPanel::alloc(mtm),
            rect,
            NSWindowStyleMask::Titled | NSWindowStyleMask::Resizable,
            NSBackingStoreType::Buffered,
            false,
        )
    };
    unsafe { panel.setReleasedWhenClosed(false) };
    panel.setMinSize(NSSize::new(520.0, 380.0));

    let root = vstack(mtm, 10.0);
    root.setEdgeInsets(NSEdgeInsets { top: 20.0, left: 20.0, bottom: 16.0, right: 20.0 });

    let head = hstack(mtm, 10.0);
    let spinner = NSProgressIndicator::new(mtm);
    spinner.setStyle(NSProgressIndicatorStyle::Spinning);
    spinner.setControlSize(NSControlSize::Small);
    spinner.setDisplayedWhenStopped(false);
    add(&head, &spinner);
    let done_icon = symbol_view(mtm, "checkmark.circle.fill", 16.0, &Palette::green());
    done_icon.setHidden(true);
    add(&head, &done_icon);
    let phase = label(mtm, "", &font(15.0, W::Semibold), &Palette::primary());
    phase.setLineBreakMode(NSLineBreakMode::ByTruncatingTail);
    phase.setContentCompressionResistancePriority_forOrientation(100.0, NSLayoutConstraintOrientation::Horizontal);
    add(&head, &phase);
    add_full(&root, &head);

    let current = label(mtm, "", &mono(11.0), &Palette::secondary());
    current.setLineBreakMode(NSLineBreakMode::ByTruncatingMiddle);
    current.setContentCompressionResistancePriority_forOrientation(100.0, NSLayoutConstraintOrientation::Horizontal);
    add_full(&root, &current);
    let counts = label(mtm, "", &digits(12.0, W::Regular), &Palette::secondary());
    add_full(&root, &counts);

    // Stall hint with a shortcut to Full Disk Access.
    let hint_box = hstack(mtm, 8.0);
    add(&hint_box, &symbol_view(mtm, "exclamationmark.triangle.fill", 13.0, &Palette::orange()));
    let hint = wrapping(mtm, "", &font(12.0, W::Regular), &Palette::orange());
    hint.setContentCompressionResistancePriority_forOrientation(100.0, NSLayoutConstraintOrientation::Horizontal);
    add(&hint_box, &hint);
    let privacy = Target::new(mtm, |_| {
        if let Some(url) = NSURL::URLWithString(&ns(
            "x-apple.systempreferences:com.apple.preference.security?Privacy_AllFiles",
        )) {
            NSWorkspace::sharedWorkspace().openURL(&url);
        }
    });
    let open_privacy = button(mtm, &tr("Mở Quyền riêng tư", "Open Privacy Settings"), &privacy);
    add(&hint_box, &open_privacy);
    hint_box.setHidden(true);
    add_full(&root, &hint_box);

    // Log
    let sv = NSTextView::scrollableTextView(mtm);
    sv.setTranslatesAutoresizingMaskIntoConstraints(false);
    sv.setBorderType(NSBorderType::BezelBorder);
    sv.heightAnchor().constraintGreaterThanOrEqualToConstant(220.0).setActive(true);
    sv.setContentHuggingPriority_forOrientation(1.0, NSLayoutConstraintOrientation::Vertical);
    let log = sv.documentView().and_then(|v| v.downcast::<NSTextView>().ok()).expect("text view");
    log.setEditable(false);
    log.setSelectable(true);
    log.setFont(Some(&mono(11.0)));
    add_full(&root, &sv);

    let buttons = hstack(mtm, 8.0);
    add(&buttons, &spacer(mtm));
    let stop_t = Target::new(mtm, |_| app().stop_scan());
    let stop = destructive_button(mtm, &tr("Dừng quét", "Stop scan"), &stop_t);
    add(&buttons, &stop);
    let close_t = Target::new(mtm, |_| crate::app::later(|a| a.close_scan_sheet()));
    let close = primary_button(mtm, &tr("Chạy nền", "Run in background"), &close_t);
    close.setKeyEquivalent(&ns("\r"));
    add(&buttons, &close);
    add_full(&root, &buttons);

    panel.setContentView(Some(&root));
    ScanSheet {
        panel,
        spinner,
        done_icon,
        phase,
        current,
        counts,
        hint_box,
        hint,
        log,
        stop,
        close,
        timer: None,
        _targets: vec![privacy, stop_t, close_t],
        shown_gen: u64::MAX,
    }
}

impl ScanSheet {
    /// Treat the log already shown as belonging to `gen` (so it is not cleared).
    pub fn keep_log_for(&mut self, gen: u64) {
        if self.shown_gen != u64::MAX {
            self.shown_gen = gen;
        }
    }
}

fn append_lines(tv: &NSTextView, lines: &[String]) {
    if lines.is_empty() {
        return;
    }
    let mut text = lines.join("\n");
    text.push('\n');
    let Some(storage) = (unsafe { tv.textStorage() }) else { return };
    unsafe {
        let attrs = NSDictionary::<NSAttributedStringKey, AnyObject>::from_slices(
            &[NSFontAttributeName, NSForegroundColorAttributeName],
            &[&*mono(11.0) as &AnyObject, &*Palette::primary() as &AnyObject],
        );
        let s = NSAttributedString::initWithString_attributes(NSAttributedString::alloc(), &ns(&text), Some(&attrs));
        storage.appendAttributedString(&s);
    }
    let len = storage.length();
    tv.scrollRangeToVisible(NSRange::new(len, 0));
}

fn clear_log(tv: &NSTextView) {
    tv.setString(&ns(""));
}

impl App {
    /// Start a scan if none is running, and show its progress.
    pub fn scan_now(&self) {
        if !self.scanning.get() {
            self.start_scan();
        }
        if self.scanning.get() || self.progress.borrow().is_some() {
            self.open_scan_sheet();
        }
    }

    pub fn open_scan_sheet(&self) {
        let Some(window) = self.window.borrow().clone() else { return };
        self.show_window();
        let mut slot = self.sheet.borrow_mut();
        let sheet = slot.get_or_insert_with(|| build(self));
        if sheet.panel.isVisible() {
            return;
        }
        unsafe { window.beginSheet_completionHandler(&sheet.panel, None) };
        if sheet.timer.is_none() {
            let tick = RcBlock::new(|_t: NonNull<NSTimer>| app().refresh_scan_sheet());
            let timer = unsafe { NSTimer::scheduledTimerWithTimeInterval_repeats_block(0.25, true, &tick) };
            unsafe { NSRunLoop::mainRunLoop().addTimer_forMode(&timer, NSRunLoopCommonModes) };
            sheet.timer = Some(timer);
        }
        drop(slot);
        self.refresh_scan_sheet();
    }

    pub fn close_scan_sheet(&self) {
        let Some(window) = self.window.borrow().clone() else { return };
        let mut slot = self.sheet.borrow_mut();
        let Some(sheet) = slot.as_mut() else { return };
        if let Some(t) = sheet.timer.take() {
            t.invalidate();
        }
        if sheet.panel.isVisible() {
            window.endSheet(&sheet.panel);
            sheet.panel.orderOut(None);
        }
    }

    /// Pull new log lines and counters from the running scan into the sheet.
    pub fn refresh_scan_sheet(&self) {
        let slot_ref = self.sheet.borrow();
        let Some(sheet) = slot_ref.as_ref() else { return };
        if !sheet.panel.isVisible() {
            return;
        }
        let Some(p) = self.progress.borrow().clone() else { return };
        let gen = self.scan_gen.get();
        drop(slot_ref);
        let mut slot = self.sheet.borrow_mut();
        let Some(sheet) = slot.as_mut() else { return };
        if sheet.shown_gen != gen {
            clear_log(&sheet.log);
            sheet.shown_gen = gen;
        }
        append_lines(&sheet.log, &p.take_log());

        let scanning = self.scanning.get();
        let dirs = p.dirs.load(Ordering::Relaxed);
        let files = p.files.load(Ordering::Relaxed);
        let found = p.found.load(Ordering::Relaxed);
        let skipped = p.skipped.load(Ordering::Relaxed);
        let (done, total) = (p.lookups_done.load(Ordering::Relaxed), p.lookups_total.load(Ordering::Relaxed));
        let elapsed = self.scan_started.get().map(|t| t.elapsed().as_secs()).unwrap_or(0);
        let phase = p.phase();
        let phase = if let Some(rest) = phase.strip_prefix("Scanning ") {
            tr(&format!("Đang quét {rest}"), &format!("Scanning {rest}"))
        } else if phase.starts_with("Checking") {
            tr(&format!("Đang kiểm tra ngày mở gần nhất ({done}/{total})"), &format!("Checking last-used dates ({done}/{total})"))
        } else {
            phase
        };

        if scanning {
            unsafe { sheet.spinner.startAnimation(None) };
            sheet.spinner.setHidden(false);
            sheet.done_icon.setHidden(true);
            sheet.phase.setStringValue(&ns(&phase));
            let cur = p.current();
            sheet.current.setStringValue(&ns(&if cur.is_empty() { String::new() } else { tr(&format!("Đang đọc: {cur}"), &format!("Reading: {cur}")) }));
            sheet.stop.setHidden(false);
            sheet.close.setTitle(&ns(&tr("Chạy nền", "Run in background")));
        } else {
            unsafe { sheet.spinner.stopAnimation(None) };
            sheet.spinner.setHidden(true);
            sheet.done_icon.setHidden(false);
            let status = self.status.borrow().clone();
            sheet.phase.setStringValue(&ns(&if status.is_empty() { tr("Đã quét xong", "Scan finished") } else { status }));
            sheet.current.setStringValue(&ns(""));
            sheet.stop.setHidden(true);
            sheet.close.setTitle(&ns(&tr("Đóng", "Close")));
        }
        sheet.counts.setStringValue(&ns(&tr(
            &format!("{dirs} thư mục · {files} tệp · {found} tệp lớn · {skipped} không đọc được · {elapsed} giây"),
            &format!("{dirs} folders · {files} files · {found} large files · {skipped} unreadable · {elapsed}s"),
        )));

        let stalled = p.stalled_for();
        let reading_folders = total == 0;
        if scanning && reading_folders && stalled >= STALL_HINT {
            let cur = p.current();
            sheet.hint.setStringValue(&ns(&tr(
                &format!(
                    "Không có tiến triển {} giây khi mở {cur}. macOS có thể đang hỏi quyền truy cập thư mục này: hãy tìm hộp thoại và bấm Cho phép, hoặc cấp Toàn quyền truy cập ổ đĩa.",
                    stalled.as_secs()
                ),
                &format!(
                    "No progress for {}s while opening {cur}. macOS may be asking for permission to access this folder: look for a dialog and click Allow, or grant Full Disk Access.",
                    stalled.as_secs()
                ),
            )));
            sheet.hint_box.setHidden(false);
        } else {
            sheet.hint_box.setHidden(true);
        }
    }
}
