//! Page builders. Each render clears the main area and builds the current page
//! from `App` state; live values are updated in place by `App::tick`.

use crate::app::*;
use crate::system;
use crate::ui::*;
use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2::{MainThreadMarker, MainThreadOnly};
use objc2_app_kit::*;
use objc2_foundation::*;
use sdc_core::format;
use sdc_core::model::*;
use sdc_core::sysstats::ProcIo;
use std::path::Path;

const SIDEBAR_W: f64 = 240.0;

// ================================================================ window shell

pub fn build_window(a: &App) -> Retained<NSWindow> {
    let mtm = a.mtm;
    let style = NSWindowStyleMask::Titled
        | NSWindowStyleMask::Closable
        | NSWindowStyleMask::Miniaturizable
        | NSWindowStyleMask::Resizable
        | NSWindowStyleMask::FullSizeContentView;
    let rect = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(1120.0, 780.0));
    let w = unsafe {
        NSWindow::initWithContentRect_styleMask_backing_defer(NSWindow::alloc(mtm), rect, style, NSBackingStoreType::Buffered, false)
    };
    unsafe { w.setReleasedWhenClosed(false) };
    w.setTitle(&ns("Safety Disk Cleaner"));
    w.setTitlebarAppearsTransparent(true);
    w.setTitleVisibility(NSWindowTitleVisibility::Hidden);
    w.setMinSize(NSSize::new(940.0, 640.0));
    w.setBackgroundColor(Some(&Palette::window()));
    w.center();
    w.setFrameAutosaveName(&ns("main"));

    let root = NSView::new(mtm);
    let sidebar = build_sidebar(a);
    root.addSubview(&sidebar);
    sidebar.leadingAnchor().constraintEqualToAnchor(&root.leadingAnchor()).setActive(true);
    sidebar.topAnchor().constraintEqualToAnchor(&root.topAnchor()).setActive(true);
    sidebar.bottomAnchor().constraintEqualToAnchor(&root.bottomAnchor()).setActive(true);
    sidebar.widthAnchor().constraintEqualToConstant(SIDEBAR_W).setActive(true);

    let main = NSView::new(mtm);
    main.setTranslatesAutoresizingMaskIntoConstraints(false);
    root.addSubview(&main);
    main.topAnchor().constraintEqualToAnchor(&root.topAnchor()).setActive(true);
    main.bottomAnchor().constraintEqualToAnchor(&root.bottomAnchor()).setActive(true);
    main.trailingAnchor().constraintEqualToAnchor(&root.trailingAnchor()).setActive(true);
    // Leading edge switches between "after sidebar" and "window edge" (setup).
    let after = main.leadingAnchor().constraintEqualToAnchor(&sidebar.trailingAnchor());
    let edge = main.leadingAnchor().constraintEqualToAnchor(&root.leadingAnchor());
    after.setActive(true);
    *a.main_leading.borrow_mut() = Some((after, edge));
    w.setContentView(Some(&root));
    *a.sidebar.borrow_mut() = Some(sidebar.into_view());
    *a.main_area.borrow_mut() = Some(main);
    w
}

/// Show or collapse the sidebar; the main area then starts at the window edge.
pub fn show_sidebar(a: &App, visible: bool) {
    if let Some(s) = a.sidebar.borrow().as_ref() {
        s.setHidden(!visible);
    }
    if let Some((after, edge)) = a.main_leading.borrow().as_ref() {
        // Deactivate first so the two constraints never conflict.
        if visible {
            edge.setActive(false);
            after.setActive(true);
        } else {
            after.setActive(false);
            edge.setActive(true);
        }
    }
}

fn sidebar_toggle(mtm: MainThreadMarker, target: &Target) -> Retained<NSButton> {
    let b = button(mtm, "", target);
    b.setBordered(false);
    with_symbol(&b, "sidebar.left");
    b.setImagePosition(NSCellImagePosition::ImageOnly);
    b.setContentTintColor(Some(&Palette::secondary()));
    let title = tr("Ẩn/hiện thanh bên", "Show/hide sidebar");
    b.setToolTip(Some(&ns(&title)));
    b.setAccessibilityLabel(Some(&ns(&title)));
    b
}

fn build_sidebar(a: &App) -> Retained<NSVisualEffectView> {
    let mtm = a.mtm;
    let fx = NSVisualEffectView::new(mtm);
    fx.setMaterial(NSVisualEffectMaterial::Sidebar);
    fx.setBlendingMode(NSVisualEffectBlendingMode::BehindWindow);
    fx.setState(NSVisualEffectState::FollowsWindowActiveState);
    fx.setTranslatesAutoresizingMaskIntoConstraints(false);

    let s = vstack(mtm, 2.0);
    s.setEdgeInsets(NSEdgeInsets { top: 52.0, left: 10.0, bottom: 14.0, right: 10.0 });
    fx.addSubview(&s);
    pin(&s, &fx, 0.0);

    // Title row, level with the window buttons: [traffic lights] name ... [toggle]
    let name = label(mtm, "SafeDiskCleaner", &font(13.0, W::Semibold), &Palette::primary());
    fx.addSubview(&name);
    name.leadingAnchor().constraintEqualToAnchor_constant(&fx.leadingAnchor(), 78.0).setActive(true);
    name.centerYAnchor().constraintEqualToAnchor_constant(&fx.topAnchor(), 15.0).setActive(true);
    let toggle = sidebar_toggle(mtm, &a.keep_target(|a| a.toggle_sidebar()));
    fx.addSubview(&toggle);
    toggle.trailingAnchor().constraintEqualToAnchor_constant(&fx.trailingAnchor(), -10.0).setActive(true);
    toggle.centerYAnchor().constraintEqualToAnchor(&name.centerYAnchor()).setActive(true);
    name.trailingAnchor().constraintLessThanOrEqualToAnchor_constant(&toggle.leadingAnchor(), -6.0).setActive(true);

    let items: [(Page, &str, Retained<NSColor>, String); 4] = [
        (Page::Overview, "square.grid.2x2.fill", Palette::gray(), tr("Tổng quan", "Overview")),
        (Page::Files, "list.bullet", Palette::green(), tr("Tệp cần xem", "Files to review")),
        (Page::Disk, "waveform.path.ecg", Palette::orange(), tr("Hoạt động đĩa", "Disk activity")),
        (Page::Volumes, "internaldrive.fill", Palette::indigo(), tr("Ổ đĩa", "Volumes")),
    ];
    for (page, sym, color, title) in items {
        add_full(&s, &nav_row(a, page, sym, &color, &title));
    }
    add_full(&s, &section(mtm, &tr("Dọn dẹp", "Cleanup")));
    add_full(&s, &nav_row(a, Page::Ignored, "eye.slash.fill", &Palette::gray(), &tr("Đã bỏ qua", "Ignored")));
    add_full(&s, &nav_row(a, Page::History, "clock.fill", &Palette::gray(), &tr("Lịch sử", "History")));
    add_full(&s, &section(mtm, &tr("Ứng dụng", "App")));
    add_full(&s, &nav_row(a, Page::Settings, "slider.horizontal.3", &Palette::gray(), &tr("Cài đặt", "Settings")));
    let sp = spacer(mtm);
    add(&s, &sp);

    let footer = hstack(mtm, 8.0);
    let text = vstack(mtm, 2.0);
    let machine = label(mtm, "Mac", &font(13.0, W::Semibold), &Palette::primary());
    let status = label(mtm, "", &font(11.0, W::Regular), &Palette::secondary());
    add(&text, &machine);
    add(&text, &status);
    add(&footer, &text);
    add(&footer, &spacer(mtm));
    add(&footer, &dot(mtm, &Palette::green(), 8.0));
    footer.setEdgeInsets(NSEdgeInsets { top: 0.0, left: 10.0, bottom: 0.0, right: 10.0 });
    add_full(&s, &footer);
    *a.footer_machine.borrow_mut() = Some(machine);
    *a.footer_status.borrow_mut() = Some(status);
    fx
}

fn section(mtm: MainThreadMarker, title: &str) -> Retained<NSView> {
    let v = NSView::new(mtm);
    v.setTranslatesAutoresizingMaskIntoConstraints(false);
    let l = label(mtm, title, &font(11.0, W::Semibold), &Palette::tertiary());
    v.addSubview(&l);
    l.leadingAnchor().constraintEqualToAnchor_constant(&v.leadingAnchor(), 10.0).setActive(true);
    l.bottomAnchor().constraintEqualToAnchor_constant(&v.bottomAnchor(), -6.0).setActive(true);
    v.heightAnchor().constraintEqualToConstant(34.0).setActive(true);
    v
}

fn nav_row(a: &App, page: Page, sym: &str, color: &NSColor, title: &str) -> Retained<ClickView> {
    let mtm = a.mtm;
    let row = ClickView::new(mtm, &NSColor::controlAccentColor(), 8.0);
    row.heightAnchor().constraintEqualToConstant(34.0).setActive(true);
    let h = hstack(mtm, 10.0);
    h.setEdgeInsets(NSEdgeInsets { top: 0.0, left: 10.0, bottom: 0.0, right: 10.0 });
    add(&h, &icon_tile(mtm, sym, color, &NSColor::whiteColor(), 22.0, 11.0));
    let t = label(mtm, title, &font(14.0, W::Regular), &Palette::primary());
    add(&h, &t);
    add(&h, &spacer(mtm));
    let metric_color = Palette::secondary();
    let m = label(mtm, "", &digits(12.0, W::Regular), &metric_color);
    m.setContentCompressionResistancePriority_forOrientation(750.0, NSLayoutConstraintOrientation::Horizontal);
    add(&h, &m);
    row.addSubview(&h);
    pin(&h, &row, 0.0);
    row.on_click(move || app().navigate(page));
    a.nav.borrow_mut().push(NavItem { page, row: row.clone(), title: t, metric: m, metric_color });
    row
}

// ================================================================ tray

pub fn build_tray(a: &App) {
    let item = NSStatusBar::systemStatusBar().statusItemWithLength(NSVariableStatusItemLength);
    if let Some(b) = item.button(a.mtm) {
        b.setImage(Some(&system::tray_image(a.worst())));
        b.setToolTip(Some(&ns("Safety Disk Cleaner")));
    }
    *a.status_item.borrow_mut() = Some(item.clone());
    fill_tray_menu(a, &item);
}

pub fn fill_tray_menu(a: &App, item: &NSStatusItem) {
    let mtm = a.mtm;
    let menu = NSMenu::new(mtm);
    menu.setAutoenablesItems(false);
    let info = |text: &str| {
        let mi = unsafe { NSMenuItem::initWithTitle_action_keyEquivalent(NSMenuItem::alloc(mtm), &ns(text), None, &ns("")) };
        mi.setEnabled(false);
        menu.addItem(&mi);
    };
    if let Some(v) = a.volumes.borrow().first() {
        info(&format!(
            "{} · {} {} · {}",
            v.name,
            format::bytes(v.free_bytes),
            tr("trống", "free"),
            level_text(v.level)
        ));
    }
    let (safe, _, review, _) = a.reclaimable();
    let n = a.active().len();
    info(&tr(
        &format!("{n} tệp có thể dọn · tối đa {}", format::bytes(safe + review)),
        &format!("{n} files to review · up to {}", format::bytes(safe + review)),
    ));
    menu.addItem(&NSMenuItem::separatorItem(mtm));
    let action = |title: &str, key: &str, f: fn(&App)| {
        let t = a.keep_target(f);
        let mi = unsafe {
            NSMenuItem::initWithTitle_action_keyEquivalent(NSMenuItem::alloc(mtm), &ns(title), Some(fire_sel()), &ns(key))
        };
        unsafe { mi.setTarget(Some(&t)) };
        menu.addItem(&mi);
    };
    action(&tr("Mở Safety Disk Cleaner", "Open Safety Disk Cleaner"), "", |a| a.show_window());
    action(&tr("Xem tệp cần dọn", "Review files"), "", |a| {
        a.show_window();
        a.navigate(Page::Files);
    });
    action(&tr("Quét ngay", "Scan now"), "r", |a| a.start_scan());
    menu.addItem(&NSMenuItem::separatorItem(mtm));
    let last = a.settings.borrow().last_scan;
    info(&format!("{} {}", tr("Lần quét:", "Last scan:"), fmt_time(&last)));
    menu.addItem(&NSMenuItem::separatorItem(mtm));
    let quit = unsafe {
        NSMenuItem::initWithTitle_action_keyEquivalent(NSMenuItem::alloc(mtm), &ns(&tr("Thoát", "Quit")), Some(objc2::sel!(terminate:)), &ns("q"))
    };
    menu.addItem(&quit);
    item.setMenu(Some(&menu));
    // Tray targets accumulate across refreshes; keep only the latest set alive.
    let mut keep = a.keep.borrow_mut();
    let n = keep.len();
    if n > 12 {
        keep.drain(0..n - 3);
    }
}

pub fn fmt_time(t: &chrono::DateTime<chrono::Utc>) -> String {
    if is_zero_time(t) {
        return "—".into();
    }
    let local = t.with_timezone(&chrono::Local);
    if local.date_naive() == chrono::Local::now().date_naive() {
        format!("{} {}", tr("hôm nay", "today"), local.format("%H:%M"))
    } else {
        local.format("%d/%m/%Y %H:%M").to_string()
    }
}

fn fmt_date(t: &chrono::DateTime<chrono::Utc>) -> String {
    t.with_timezone(&chrono::Local).format("%d/%m/%Y").to_string()
}

// ================================================================ render entry

pub fn render(a: &App) {
    let Some(main) = a.main_area.borrow().clone() else { return };
    // Old page targets die only now, outside any control callback.
    let old_targets = std::mem::take(&mut *a.page_targets.borrow_mut());
    *a.refs.borrow_mut() = Refs::default();
    for v in main.subviews().iter() {
        v.removeFromSuperview();
    }
    drop(old_targets);

    if !a.settings.borrow().setup_done {
        show_sidebar(a, false);
        let page = setup_page(a);
        main.addSubview(&page);
        pin(&page, &main, 0.0);
        return;
    }
    show_sidebar(a, !a.sidebar_collapsed.get());
    let body = match a.page.get() {
        Page::Overview => overview_page(a),
        Page::Files => files_page(a, false),
        Page::Ignored => files_page(a, true),
        Page::Disk => disk_page(a),
        Page::Volumes => volumes_page(a),
        Page::History => history_page(a),
        Page::Settings => settings_page(a),
    };
    main.addSubview(&body);
    pin(&body, &main, 0.0);
    a.tick();
    a.sync_selection();
}

/// Page frame: header (title, subtitle, accessories) above a body view.
fn frame(a: &App, title: &str, subtitle: &str, accessories: Vec<Retained<NSView>>, body: &NSView) -> Retained<NSView> {
    let mtm = a.mtm;
    let root = vstack(mtm, 6.0);
    root.setEdgeInsets(NSEdgeInsets { top: 12.0, left: 0.0, bottom: 0.0, right: 0.0 });
    let header = hstack(mtm, 10.0);
    let collapsed = a.sidebar_collapsed.get();
    header.setEdgeInsets(NSEdgeInsets { top: 0.0, left: if collapsed { 84.0 } else { 24.0 }, bottom: 4.0, right: 24.0 });
    if collapsed {
        add(&header, &sidebar_toggle(mtm, &a.target(|a| a.toggle_sidebar())));
    }
    let titles = vstack(mtm, 1.0);
    add(&titles, &label(mtm, title, &font(17.0, W::Bold), &Palette::primary()));
    let status = a.status.borrow().clone();
    let (sub, sub_color) =
        if status.is_empty() { (subtitle.to_string(), Palette::secondary()) } else { (status, Palette::blue()) };
    add(&titles, &label(mtm, &sub, &font(12.0, W::Regular), &sub_color));
    add(&header, &titles);
    add(&header, &spacer(mtm));
    for acc in accessories {
        add(&header, &acc);
    }
    header.heightAnchor().constraintGreaterThanOrEqualToConstant(40.0).setActive(true);
    add_full(&root, &header);
    root.addArrangedSubview(body);
    body.leadingAnchor().constraintEqualToAnchor(&root.leadingAnchor()).setActive(true);
    body.trailingAnchor().constraintEqualToAnchor(&root.trailingAnchor()).setActive(true);
    body.setContentHuggingPriority_forOrientation(1.0, NSLayoutConstraintOrientation::Vertical);
    root.into_super()
}

/// Scrolling page body: vertical stack with page gutters.
fn page_stack(mtm: MainThreadMarker) -> Retained<NSStackView> {
    let s = vstack(mtm, 16.0);
    s.setEdgeInsets(NSEdgeInsets { top: 6.0, left: 24.0, bottom: 24.0, right: 24.0 });
    s
}

fn scan_button(a: &App) -> Retained<NSView> {
    let mtm = a.mtm;
    if a.scanning.get() {
        let h = hstack(mtm, 8.0);
        let spin = NSProgressIndicator::new(mtm);
        spin.setStyle(NSProgressIndicatorStyle::Spinning);
        spin.setControlSize(NSControlSize::Small);
        unsafe { spin.startAnimation(None) };
        add(&h, &spin);
        let stop = button(mtm, &tr("Dừng", "Stop"), &a.target(|a| a.cancel.store(true, std::sync::atomic::Ordering::Relaxed)));
        add(&h, &stop);
        return h.into_view();
    }
    let b = primary_button(mtm, &tr("Quét ngay", "Scan now"), &a.target(|a| a.start_scan()));
    with_symbol(&b, "arrow.clockwise");
    b.into_view()
}

fn card_header(mtm: MainThreadMarker, sym: &str, color: &NSColor, title: &str, trailing: Option<Retained<NSView>>) -> Retained<NSStackView> {
    let h = hstack(mtm, 10.0);
    add(&h, &icon_tile(mtm, sym, &Palette::tint(color, 0.18), color, 28.0, 13.0));
    add(&h, &label(mtm, title, &font(17.0, W::Semibold), color));
    add(&h, &spacer(mtm));
    if let Some(t) = trailing {
        add(&h, &t);
    }
    h
}

// ================================================================ overview

fn overview_page(a: &App) -> Retained<NSView> {
    let mtm = a.mtm;
    let s = page_stack(mtm);
    let (safe, n_safe, review, n_review) = a.reclaimable();
    let vols = a.volumes.borrow().clone();
    if vols.is_empty() {
        let (c, inner) = card(mtm, 20.0, 8.0);
        add(&inner, &label(mtm, &tr("Chưa đọc được thông tin ổ đĩa.", "Disk information is unavailable."), &font(13.0, W::Regular), &Palette::secondary()));
        add_full(&s, &c);
    }
    for (i, v) in vols.iter().enumerate() {
        add_full(&s, &volume_card(a, v, if i == 0 { safe + review } else { 0 }, true));
    }

    let row = columns(mtm, 16.0);
    // Cleanup suggestions
    let (c1, inner) = card(mtm, 20.0, 6.0);
    add_full(&inner, &card_header(mtm, "checkmark.shield.fill", &Palette::green(), &tr("Gợi ý dọn dẹp", "Cleanup suggestions"), None));
    inner.setCustomSpacing_afterView(14.0, &inner.arrangedSubviews().lastObject().unwrap());
    add(&inner, &label(mtm, &tr("Có thể giải phóng tối đa", "Up to"), &font(13.0, W::Regular), &Palette::secondary()));
    let (n, u) = format::bytes_parts(safe + review);
    add(&inner, &hero(mtm, &n, &u, 40.0));
    let total = (safe + review).max(1) as f64;
    let split = Bar::new(mtm, 8.0, vec![(0.0, safe as f64 / total, Palette::green()), (safe as f64 / total, 1.0, Palette::yellow())], 2.0);
    add_full(&inner, &split);
    let lg = hstack(mtm, 16.0);
    add(&lg, &legend(mtm, &Palette::green(), &tr(&format!("{n_safe} an toàn · {}", format::bytes(safe)), &format!("{n_safe} safe · {}", format::bytes(safe)))));
    add(&lg, &legend(mtm, &Palette::yellow(), &tr(&format!("{n_review} cần xem · {}", format::bytes(review)), &format!("{n_review} to review · {}", format::bytes(review)))));
    add(&inner, &lg);
    add(&inner, &spacer(mtm));
    let foot = hstack(mtm, 8.0);
    add(&foot, &label(mtm, &tr("Chỉ giải phóng sau khi dọn Thùng rác", "Reclaimed after emptying the Trash"), &font(12.0, W::Regular), &Palette::tertiary()));
    add(&foot, &spacer(mtm));
    let count = n_safe + n_review;
    add(&foot, &link_button(mtm, &tr(&format!("Xem {count} tệp"), &format!("Review {count} files")), &Palette::green(), &a.target(|a| a.navigate(Page::Files))));
    add_full(&inner, &foot);
    add(&row, &c1);

    // Mini disk activity
    let (c2, inner) = card(mtm, 20.0, 10.0);
    let lg = hstack(mtm, 12.0);
    add(&lg, &legend(mtm, &Palette::orange(), &tr("Ghi", "Write")));
    add(&lg, &legend(mtm, &Palette::cyan(), &tr("Đọc", "Read")));
    add_full(&inner, &card_header(mtm, "waveform.path.ecg", &Palette::orange(), &tr("Hoạt động đĩa", "Disk activity"), Some(lg.into_view())));
    let stats = hstack(mtm, 28.0);
    stats.setAlignment(NSLayoutAttribute::LastBaseline);
    let rate = hero(mtm, "0", "MB/s", 40.0);
    add(&stats, &rate);
    let (r, rl) = stat(mtm, &tr("Đọc", "Read"), "—", None);
    let (w, wl) = stat(mtm, &tr("Ghi", "Write"), "—", None);
    add(&stats, &r);
    add(&stats, &w);
    add(&inner, &stats);
    let chart = DotChart::new(mtm, 7.0, 4.0, 62.0);
    add_full(&inner, &chart_box(mtm, &chart));
    add(&row, &c2);
    c1.heightAnchor().constraintEqualToAnchor(&c2.heightAnchor()).setActive(true);
    add_full(&s, &row);
    {
        let mut refs = a.refs.borrow_mut();
        refs.rate_total = Some(rate);
        refs.rate_read = Some(rl);
        refs.rate_write = Some(wl);
        refs.chart = Some(chart);
    }

    // Largest files
    let mut top = a.active();
    top.sort_by_key(|c| std::cmp::Reverse(c.size));
    top.truncate(8);
    if !top.is_empty() {
        let (c3, inner) = card(mtm, 18.0, 8.0);
        let more = link_button(mtm, &tr("Xem tất cả", "Show all"), &Palette::blue(), &a.target(|a| a.navigate(Page::Files)));
        add_full(&inner, &card_header(mtm, "doc.fill", &Palette::blue(), &tr("Tệp lớn nhất", "Largest files"), Some(more.into_view())));
        let grid = columns(mtm, 48.0);
        let left = vstack(mtm, 0.0);
        let right = vstack(mtm, 0.0);
        for (i, c) in top.iter().enumerate() {
            let col = if i % 2 == 0 { &left } else { &right };
            add_full(col, &file_line(a, c));
        }
        add(&grid, &left);
        add(&grid, &right);
        add_full(&inner, &grid);
        add_full(&s, &c3);
    }
    let subtitle = format!(
        "{} {} · {} {}",
        tr("Lần quét gần nhất:", "Last scan:"),
        fmt_time(&a.settings.borrow().last_scan),
        vols.len(),
        tr("ổ đĩa", if vols.len() == 1 { "volume" } else { "volumes" })
    );
    let sc = scroller(mtm, &s);
    frame(a, &tr("Tổng quan", "Overview"), &subtitle, vec![scan_button(a)], &sc)
}

fn file_line(a: &App, c: &Candidate) -> Retained<NSView> {
    let mtm = a.mtm;
    let h = hstack(mtm, 10.0);
    h.heightAnchor().constraintEqualToConstant(32.0).setActive(true);
    add(&h, &dot(mtm, &safety_color(c), 8.0));
    let p = Path::new(&c.path);
    let name = label(mtm, &p.file_name().unwrap_or_default().to_string_lossy(), &font(14.0, W::Regular), &Palette::primary());
    add(&h, &name);
    let dir = p.parent().and_then(|d| d.file_name()).map(|d| d.to_string_lossy().into_owned()).unwrap_or_default();
    add(&h, &label(mtm, &dir, &font(12.0, W::Regular), &Palette::tertiary()));
    add(&h, &spacer(mtm));
    let size = label(mtm, &format::bytes(c.size), &digits(14.0, W::Semibold), &Palette::primary());
    size.setContentCompressionResistancePriority_forOrientation(800.0, NSLayoutConstraintOrientation::Horizontal);
    add(&h, &size);
    h.into_view()
}

fn safety_color(c: &Candidate) -> Retained<NSColor> {
    match (c.ignored, c.safety) {
        (true, _) => Palette::gray(),
        (_, Safety::Safe) => Palette::green(),
        (_, Safety::Protected) => Palette::gray(),
        _ => Palette::yellow(),
    }
}

fn chart_box(mtm: MainThreadMarker, chart: &DotChart) -> Retained<NSBox> {
    let b = boxed(mtm, &Palette::tint(&Palette::orange(), 0.07), 10.0);
    b.addSubview(chart);
    pin(chart, &b, 10.0);
    b
}

fn volume_card(a: &App, v: &Volume, reclaimable: i64, full: bool) -> Retained<NSBox> {
    let mtm = a.mtm;
    let (c, inner) = card(mtm, 20.0, 10.0);
    let lv = level_color(v.level);
    let pill: Retained<NSView> = badge(mtm, &level_text(v.level), &lv).into_view();
    add_full(&inner, &card_header(mtm, "internaldrive.fill", &Palette::orange(), &v.name, Some(pill)));
    let row = if full { hstack(mtm, 40.0) } else { vstack(mtm, 12.0) };
    if full {
        row.setAlignment(NSLayoutAttribute::Bottom);
    }
    let left = vstack(mtm, 2.0);
    add(&left, &label(mtm, &tr(&format!("Còn trống trên {}", format::bytes(v.total_bytes)), &format!("Free of {}", format::bytes(v.total_bytes))), &font(13.0, W::Regular), &Palette::secondary()));
    let (n, u) = format::bytes_parts(v.free_bytes);
    add(&left, &hero(mtm, &n, &u, if full { 44.0 } else { 34.0 }));
    add(&row, &left);
    let grid = columns(mtm, 16.0);
    add(&grid, &stat(mtm, &tr("Đã dùng", "Used"), &format::bytes(v.total_bytes - v.free_bytes), None).0);
    add(&grid, &stat(mtm, &tr("Định dạng", "Format"), &v.fs_type, None).0);
    add(&grid, &stat(mtm, &tr("Thiết bị", "Device"), &v.bsd_name, None).0);
    if full {
        let s = a.settings.borrow();
        add(&grid, &stat(mtm, &tr("Cảnh báo", "Alerts"), &format!("≤ {} · ≤ {} GB", s.yellow_gb, s.red_gb), None).0);
    }
    add(&row, &grid);
    add_full(&inner, &row);
    grid.trailingAnchor().constraintEqualToAnchor_constant(&row.trailingAnchor(), 0.0).setActive(true);
    let total = v.total_bytes.max(1) as f64;
    let used = (v.total_bytes - v.free_bytes) as f64 / total;
    let rec = (reclaimable as f64 / total).min(used);
    let mut segs = vec![(0.0, used, Palette::orange())];
    if rec > 0.0 {
        segs.push((used - rec, used, Palette::green()));
    }
    inner.setCustomSpacing_afterView(16.0, &row);
    add_full(&inner, &Bar::new(mtm, 8.0, segs, 0.0));
    if full {
        let lg = hstack(mtm, 18.0);
        add(&lg, &legend(mtm, &Palette::orange(), &tr("Đã dùng", "Used")));
        if reclaimable > 0 {
            add(&lg, &legend(mtm, &Palette::green(), &tr(&format!("Có thể giải phóng {}", format::bytes(reclaimable)), &format!("Reclaimable {}", format::bytes(reclaimable)))));
        }
        add(&lg, &legend(mtm, &Palette::track(), &tr("Trống", "Free")));
        add(&inner, &lg);
    }
    c
}

// ================================================================ disk activity

fn disk_page(a: &App) -> Retained<NSView> {
    let mtm = a.mtm;
    let s = page_stack(mtm);
    let (c, inner) = card(mtm, 20.0, 12.0);
    let lg = hstack(mtm, 14.0);
    add(&lg, &legend(mtm, &Palette::orange(), &tr("Ghi", "Write")));
    add(&lg, &legend(mtm, &Palette::cyan(), &tr("Đọc", "Read")));
    add_full(&inner, &card_header(mtm, "waveform.path.ecg", &Palette::orange(), &tr("Đọc và ghi", "Reading and writing"), Some(lg.into_view())));
    let stats = hstack(mtm, 44.0);
    stats.setAlignment(NSLayoutAttribute::Bottom);
    let total = vstack(mtm, 2.0);
    add(&total, &label(mtm, &tr("Tổng tốc độ", "Total"), &font(13.0, W::Regular), &Palette::secondary()));
    let rate = hero(mtm, "0", "MB/s", 44.0);
    add(&total, &rate);
    add(&stats, &total);
    let (r, rl) = stat(mtm, &tr("Đọc", "Reading"), "—", Some(" "));
    let (w, wl) = stat(mtm, &tr("Ghi", "Writing"), "—", Some(" "));
    let (rt, rtl) = stat(mtm, &tr("Đã đọc", "Read"), "—", Some(&tr("từ lúc khởi động", "since startup")));
    let (wt, wtl) = stat(mtm, &tr("Đã ghi", "Written"), "—", Some(&tr("từ lúc khởi động", "since startup")));
    for v in [&r, &w, &rt, &wt] {
        add(&stats, v);
    }
    add(&inner, &stats);
    let chart = DotChart::new(mtm, 7.0, 4.0, 112.0);
    let chart_wrap = vstack(mtm, 6.0);
    add_full(&chart_wrap, &chart_box(mtm, &chart));
    let axis = hstack(mtm, 0.0);
    axis.setDistribution(NSStackViewDistribution::EqualSpacing);
    let labels: &[&str] = match a.range.get() {
        0 => &["−1p", "−45g", "−30g", "−15g"],
        1 => &["−5p", "−4p", "−3p", "−2p", "−1p"],
        2 => &["−15p", "−10p", "−5p"],
        _ => &["−60p", "−45p", "−30p", "−15p"],
    };
    let mut labels: Vec<String> = labels.iter().map(|l| if crate::app::tr("v", "e") == "v" { l.to_string() } else { l.replace('p', "m").replace('g', "s") }).collect();
    labels.push(tr("bây giờ", "now"));
    for l in labels {
        add(&axis, &label(mtm, &l, &font(11.0, W::Regular), &Palette::tertiary()));
    }
    add_full(&chart_wrap, &axis);
    add_full(&inner, &chart_wrap);
    add_full(&s, &c);

    // Volumes + disk images
    let row = columns(mtm, 16.0);
    let vols = a.volumes.borrow().clone();
    if let Some(v) = vols.first() {
        add(&row, &volume_card(a, v, 0, false));
    }
    let images: Vec<_> = a
        .live
        .lock()
        .map(|l| l.images.clone())
        .unwrap_or_default()
        .into_iter()
        .filter(|i| i.mount_point.as_deref().is_some_and(|m| m.starts_with("/Volumes/")))
        .collect();
    let (ic, inner) = card(mtm, 18.0, 8.0);
    let n_mounted = images.iter().filter(|i| i.mount_point.is_some()).count();
    let count = label(mtm, &tr(&format!("{n_mounted} đang gắn"), &format!("{n_mounted} mounted")), &font(13.0, W::Regular), &Palette::secondary());
    add_full(&inner, &card_header(mtm, "square.stack.3d.up.fill", &Palette::gray(), &tr("Ảnh đĩa", "Disk images"), Some(count.into_view())));
    if images.is_empty() {
        add(&inner, &label(mtm, &tr("Không có ảnh đĩa nào đang gắn.", "No disk images mounted."), &font(13.0, W::Regular), &Palette::tertiary()));
    }
    for img in images.iter().take(6) {
        let h = hstack(mtm, 8.0);
        add(&h, &label(mtm, &img.name, &font(14.0, W::Regular), &Palette::secondary()));
        add(&h, &spacer(mtm));
        let v = label(mtm, &format!("{} · {}", format::bytes(img.size as i64), img.fs_type), &digits(14.0, W::Semibold), &Palette::primary());
        v.setContentCompressionResistancePriority_forOrientation(800.0, NSLayoutConstraintOrientation::Horizontal);
        add(&h, &v);
        add_full(&inner, &h);
    }
    add(&row, &ic);
    add_full(&s, &row);

    // Top processes
    let (pc, inner) = card(mtm, 18.0, 8.0);
    add_full(&inner, &card_header(mtm, "list.bullet", &Palette::orange(), &tr("Đọc/ghi nhiều nhất", "Reading and writing the most"), None));
    let procs = columns(mtm, 48.0);
    add_full(&inner, &procs);
    add_full(&s, &pc);

    {
        let mut refs = a.refs.borrow_mut();
        refs.rate_total = Some(rate);
        refs.rate_read = Some(rl);
        refs.rate_write = Some(wl);
        refs.read_total = Some(rtl);
        refs.written_total = Some(wtl);
        refs.chart = Some(chart);
        refs.procs = Some(procs);
    }
    let seg = segmented(mtm, &[&tr("1p", "1m"), &tr("5p", "5m"), &tr("15p", "15m"), &tr("1g", "1h")], a.range.get(), &a.target(|_| {}));
    let seg_ref = seg.clone();
    let t = a.target(move |a| {
        a.range.set(seg_ref.selectedSegment());
        a.request_render();
    });
    unsafe {
        seg.setTarget(Some(&t));
    }
    let pause = button(mtm, "", &a.target(|a| {
        a.paused.set(!a.paused.get());
        a.request_render();
    }));
    with_symbol(&pause, if a.paused.get() { "play.fill" } else { "pause.fill" });
    pause.setBezelStyle(NSBezelStyle::Circular);
    let n_mounts = a.live.lock().map(|l| l.mounts.len()).unwrap_or(0).max(vols.len());
    let subtitle = tr(&format!("{n_mounts} ổ · cập nhật mỗi giây"), &format!("{n_mounts} volumes · updated every second"));
    let sc = scroller(mtm, &s);
    frame(a, &tr("Hoạt động đĩa", "Disk activity"), &subtitle, vec![seg.into_view(), pause.into_view()], &sc)
}

/// Rebuild the top-process columns (two columns, up to 10 rows).
pub fn fill_procs(a: &App, stack: &NSStackView, procs: &[ProcIo]) {
    let mtm = a.mtm;
    for v in stack.arrangedSubviews().iter() {
        stack.removeArrangedSubview(&v);
        v.removeFromSuperview();
    }
    let left = vstack(mtm, 0.0);
    let right = vstack(mtm, 0.0);
    if procs.is_empty() {
        add(&left, &label(mtm, &tr("Đang đo…", "Measuring…"), &font(13.0, W::Regular), &Palette::tertiary()));
    }
    for (i, p) in procs.iter().take(10).enumerate() {
        let h = hstack(mtm, 10.0);
        h.heightAnchor().constraintEqualToConstant(30.0).setActive(true);
        let icon = match p.app_path.as_deref() {
            Some(path) => {
                let iv = NSImageView::imageViewWithImage(&system::app_icon(path), mtm);
                iv.setTranslatesAutoresizingMaskIntoConstraints(false);
                iv
            }
            None => symbol_view(mtm, "gearshape.fill", 13.0, &Palette::gray()),
        };
        size(&icon, 20.0, 20.0);
        add(&h, &icon);
        add(&h, &label(mtm, &p.name, &font(14.0, W::Regular), &Palette::primary()));
        add(&h, &label(mtm, &p.processes.to_string(), &font(12.0, W::Regular), &Palette::tertiary()));
        add(&h, &spacer(mtm));
        let r = label(mtm, &format::rate(p.rate), &digits(14.0, W::Semibold), &Palette::primary());
        r.setContentCompressionResistancePriority_forOrientation(800.0, NSLayoutConstraintOrientation::Horizontal);
        add(&h, &r);
        add_full(if i % 2 == 0 { &left } else { &right }, &h);
    }
    add(stack, &left);
    add(stack, &right);
}

// ================================================================ files

fn files_page(a: &App, ignored_page: bool) -> Retained<NSView> {
    let mtm = a.mtm;
    let query = a.filter.borrow().to_lowercase();
    let tab = a.tab.get();
    let mut items: Vec<Candidate> = a
        .candidates
        .borrow()
        .iter()
        .filter(|c| c.ignored == ignored_page)
        .filter(|c| query.is_empty() || c.path.to_lowercase().contains(&query))
        .filter(|c| ignored_page || tab == 0 || (tab == 1) == (c.safety == Safety::Safe))
        .cloned()
        .collect();
    if a.sort_small.get() {
        items.sort_by_key(|c| c.size);
    } else {
        items.sort_by_key(|c| std::cmp::Reverse(c.size));
    }

    let body = vstack(mtm, 0.0);
    body.setEdgeInsets(NSEdgeInsets { top: 6.0, left: 24.0, bottom: 24.0, right: 24.0 });
    let (c, inner) = card(mtm, 0.0, 0.0);
    // Column header
    let head = table_row_layout(mtm);
    let all_ids: Vec<String> = items.iter().map(|c| c.id.clone()).collect();
    let ids_for_all = all_ids.clone();
    let all = unsafe {
        NSButton::checkboxWithTitle_target_action(&ns(""), Some(&a.target(move |a| {
            let every = ids_for_all.iter().all(|id| a.selected.borrow().contains(id));
            a.set_all(&ids_for_all, !every);
        })), Some(fire_sel()), mtm)
    };
    let hdr = |t: &str| label(mtm, t, &font(12.0, W::Semibold), &Palette::secondary());
    let size_title = format!("{} {}", tr("Kích thước", "Size"), if a.sort_small.get() { "▴" } else { "▾" });
    let size_btn = unsafe {
        NSButton::buttonWithTitle_target_action(&ns(&size_title), Some(&a.target(|a| {
            a.sort_small.set(!a.sort_small.get());
            a.request_render();
        })), Some(fire_sel()), mtm)
    };
    size_btn.setBordered(false);
    size_btn.setFont(Some(&font(12.0, W::Semibold)));
    size_btn.setAlignment(NSTextAlignment::Right);
    fill_row(
        &head,
        all.into_view(),
        hdr(&tr("Tên", "Name")).into_view(),
        hdr(&tr("Lý do", "Reason")).into_view(),
        hdr(&tr("Dùng lần cuối", "Last used")).into_view(),
        size_btn.into_view(),
        hdr(&tr("Trạng thái", "Status")).into_view(),
    );
    let head_wrap = NSView::new(mtm);
    head_wrap.setTranslatesAutoresizingMaskIntoConstraints(false);
    head_wrap.addSubview(&head);
    pin(&head, &head_wrap, 0.0);
    head_wrap.heightAnchor().constraintEqualToConstant(36.0).setActive(true);
    add_full(&inner, &head_wrap);
    add_full(&inner, &separator(mtm));

    // Rows
    let list = vstack(mtm, 0.0);
    let mut rows = vec![];
    if items.is_empty() {
        let empty = vstack(mtm, 6.0);
        empty.setEdgeInsets(NSEdgeInsets { top: 40.0, left: 16.0, bottom: 40.0, right: 16.0 });
        empty.setAlignment(NSLayoutAttribute::CenterX);
        add(&empty, &symbol_view(mtm, if ignored_page { "eye.slash" } else { "checkmark.circle" }, 28.0, &Palette::tertiary()));
        add(&empty, &label(mtm, &if ignored_page { tr("Không có tệp nào bị bỏ qua.", "No ignored files.") } else { tr("Không có tệp phù hợp.", "No matching files.") }, &font(14.0, W::Regular), &Palette::secondary()));
        add_full(&list, &empty);
    }
    for item in items.iter().take(1000) {
        let (row, check) = file_row(a, item);
        add_full(&list, &row);
        add_full(&list, &separator(mtm));
        rows.push((item.id.clone(), row, check));
    }
    let sc = scroller(mtm, &list);
    sc.setContentHuggingPriority_forOrientation(1.0, NSLayoutConstraintOrientation::Vertical);
    add_full(&inner, &sc);

    // Action bar
    let bar_box = boxed(mtm, &Palette::card_alt(), 0.0);
    let bar = hstack(mtm, 8.0);
    bar.setEdgeInsets(NSEdgeInsets { top: 12.0, left: 16.0, bottom: 12.0, right: 16.0 });
    let txt = vstack(mtm, 1.0);
    let sel_label = label(mtm, "", &font(14.0, W::Semibold), &Palette::primary());
    add(&txt, &sel_label);
    add(&txt, &label(mtm, &tr("Mỗi tệp được kiểm tra lại trước khi dọn", "Every file is re-checked before cleanup"), &font(11.0, W::Regular), &Palette::tertiary()));
    add(&bar, &txt);
    add(&bar, &spacer(mtm));
    let reveal = button(mtm, &tr("Mở trong Finder", "Reveal in Finder"), &a.target(|a| a.reveal_selected()));
    let ignore = if ignored_page {
        button(mtm, &tr("Đưa lại danh sách", "Restore"), &a.target(|a| a.ignore_selected(false)))
    } else {
        button(mtm, &tr("Bỏ qua", "Ignore"), &a.target(|a| a.ignore_selected(true)))
    };
    let delete = destructive_button(mtm, &tr("Xoá vĩnh viễn…", "Delete permanently…"), &a.target(|_| later(|a| a.confirm_delete())));
    let trash = primary_button(mtm, &tr("Chuyển vào Thùng rác", "Move to Trash"), &a.target(|_| later(|a| a.confirm_trash())));
    with_symbol(&trash, "trash");
    for b in [&reveal, &ignore, &delete, &trash] {
        add(&bar, b);
    }
    bar_box.addSubview(&bar);
    pin(&bar, &bar_box, 0.0);
    add_full(&inner, &separator(mtm));
    add_full(&inner, &bar_box);
    add_full(&body, &c);
    c.bottomAnchor().constraintEqualToAnchor_constant(&body.bottomAnchor(), -24.0).setActive(true);
    {
        let mut refs = a.refs.borrow_mut();
        refs.rows = rows;
        refs.sel_label = Some(sel_label);
        refs.sel_buttons = vec![reveal, ignore, delete, trash];
    }

    // Header accessories: search + filter
    let search = NSSearchField::new(mtm);
    search.setPlaceholderString(Some(&ns(&tr("Lọc theo đường dẫn", "Filter by path"))));
    search.setStringValue(&ns(&a.filter.borrow()));
    search.setTranslatesAutoresizingMaskIntoConstraints(false);
    search.widthAnchor().constraintEqualToConstant(220.0).setActive(true);
    let search_ref = search.clone();
    let st = a.target(move |a| {
        *a.filter.borrow_mut() = search_ref.stringValue().to_string();
        a.request_render();
    });
    unsafe {
        search.setTarget(Some(&st));
        search.setAction(Some(fire_sel()));
    }
    search.setSendsWholeSearchString(true);
    let mut acc: Vec<Retained<NSView>> = vec![search.into_view()];
    let (safe_n, review_n) = {
        let c = a.candidates.borrow();
        (
            c.iter().filter(|c| !c.ignored && c.safety == Safety::Safe).count(),
            c.iter().filter(|c| !c.ignored && c.safety != Safety::Safe).count(),
        )
    };
    if !ignored_page {
        let labels = [
            tr(&format!("Tất cả {}", safe_n + review_n), &format!("All {}", safe_n + review_n)),
            tr(&format!("An toàn {safe_n}"), &format!("Safe {safe_n}")),
            tr(&format!("Cần xem {review_n}"), &format!("Review {review_n}")),
        ];
        let seg = segmented(mtm, &[&labels[0], &labels[1], &labels[2]], tab, &a.target(|_| {}));
        let seg_ref = seg.clone();
        let t = a.target(move |a| {
            a.tab.set(seg_ref.selectedSegment());
            a.request_render();
        });
        unsafe { seg.setTarget(Some(&t)) };
        acc.push(seg.into_view());
    }
    let total: i64 = items.iter().map(|c| c.size).sum();
    let (title, subtitle) = if ignored_page {
        (tr("Đã bỏ qua", "Ignored"), tr(&format!("{} tệp không còn được gợi ý", items.len()), &format!("{} files no longer suggested", items.len())))
    } else {
        (
            tr("Tệp cần xem", "Files to review"),
            tr(&format!("{} tệp · tối đa {}", items.len(), format::bytes(total)), &format!("{} files · up to {}", items.len(), format::bytes(total))),
        )
    };
    frame(a, &title, &subtitle, acc, &body)
}

fn table_row_layout(mtm: MainThreadMarker) -> Retained<NSStackView> {
    let h = hstack(mtm, 12.0);
    h.setDistribution(NSStackViewDistribution::Fill);
    h.setEdgeInsets(NSEdgeInsets { top: 0.0, left: 16.0, bottom: 0.0, right: 16.0 });
    h
}

/// Lay out the six table columns with fixed widths for the narrow ones.
fn fill_row(
    h: &NSStackView,
    check: Retained<NSView>,
    name: Retained<NSView>,
    reason: Retained<NSView>,
    used: Retained<NSView>,
    size_v: Retained<NSView>,
    status: Retained<NSView>,
) {
    size(&check, 18.0, 0.0);
    add(h, &check);
    name.setContentHuggingPriority_forOrientation(1.0, NSLayoutConstraintOrientation::Horizontal);
    reason.setContentHuggingPriority_forOrientation(2.0, NSLayoutConstraintOrientation::Horizontal);
    name.setContentCompressionResistancePriority_forOrientation(100.0, NSLayoutConstraintOrientation::Horizontal);
    reason.setContentCompressionResistancePriority_forOrientation(100.0, NSLayoutConstraintOrientation::Horizontal);
    add(h, &name);
    add(h, &reason);
    reason.widthAnchor().constraintEqualToAnchor_multiplier(&name.widthAnchor(), 0.9).setActive(true);
    size(&used, 96.0, 0.0);
    add(h, &used);
    size(&size_v, 84.0, 0.0);
    add(h, &size_v);
    let wrap = hstack(h.mtm(), 0.0);
    add(&wrap, &spacer(h.mtm()));
    add(&wrap, &status);
    size(&wrap, 110.0, 0.0);
    add(h, &wrap);
}

fn file_row(a: &App, c: &Candidate) -> (Retained<ClickView>, Retained<NSButton>) {
    let mtm = a.mtm;
    let row = ClickView::new(mtm, &Palette::tint(&NSColor::controlAccentColor(), 0.14), 0.0);
    row.heightAnchor().constraintEqualToConstant(54.0).setActive(true);
    let id = c.id.clone();
    let id2 = c.id.clone();
    row.on_click(move || app().toggle(&id));
    let check = unsafe { NSButton::checkboxWithTitle_target_action(&ns(""), Some(&a.target(move |a| a.toggle(&id2))), Some(fire_sel()), mtm) };
    let h = table_row_layout(mtm);
    let p = Path::new(&c.path);
    let name_col = vstack(mtm, 2.0);
    add(&name_col, &label(mtm, &p.file_name().unwrap_or_default().to_string_lossy(), &font(14.0, W::Semibold), &Palette::primary()));
    let dir = p.parent().map(|d| d.to_string_lossy().replace(&*a.home.to_string_lossy(), "~")).unwrap_or_default();
    let dl = label(mtm, &dir, &font(12.0, W::Regular), &Palette::tertiary());
    dl.setLineBreakMode(NSLineBreakMode::ByTruncatingMiddle);
    add(&name_col, &dl);
    let reason = hstack(mtm, 6.0);
    add(&reason, &symbol_view(mtm, "sparkles", 11.0, &Palette::purple()));
    let rl = label(mtm, &c.reason, &font(12.0, W::Regular), &Palette::secondary());
    rl.setToolTip(Some(&ns(&c.reason)));
    add(&reason, &rl);
    let used = label(
        mtm,
        &if c.last_use_known { fmt_date(&c.last_used_at) } else { tr("Không rõ", "Unknown") },
        &digits(13.0, W::Regular),
        &Palette::secondary(),
    );
    let sz = label(mtm, &format::bytes(c.size), &digits(14.0, W::Semibold), &Palette::primary());
    sz.setAlignment(NSTextAlignment::Right);
    let (text, color) = match (c.ignored, c.safety) {
        (true, _) => (tr("Đã bỏ qua", "Ignored"), Palette::gray()),
        (_, Safety::Safe) => (tr("Gợi ý an toàn", "Safe suggestion"), Palette::green()),
        (_, Safety::Protected) => (tr("Được bảo vệ", "Protected"), Palette::gray()),
        _ => (tr("Cần xem", "Review"), Palette::yellow()),
    };
    fill_row(
        &h,
        check.clone().into_view(),
        name_col.into_view(),
        reason.into_view(),
        used.into_view(),
        sz.into_view(),
        badge(mtm, &text, &color).into_view(),
    );
    row.addSubview(&h);
    pin(&h, &row, 0.0);
    (row, check)
}

// ================================================================ volumes

fn volumes_page(a: &App) -> Retained<NSView> {
    let mtm = a.mtm;
    let s = page_stack(mtm);
    let settings = a.settings.borrow().clone();
    let mounts = a.live.lock().map(|l| l.mounts.clone()).unwrap_or_default();
    let mut vols: Vec<Volume> = mounts
        .iter()
        .map(|m| Volume {
            path: m.mount_point.clone(),
            name: sdc_core::scan::volume_name(&m.mount_point),
            device: 0,
            free_bytes: m.free,
            total_bytes: m.total,
            fs_type: m.fs_type.to_uppercase(),
            bsd_name: m.device.clone(),
            level: level(m.free, settings.red_gb, settings.yellow_gb),
        })
        .collect();
    if vols.is_empty() {
        vols = a.volumes.borrow().clone();
    }
    let (safe, _, review, _) = a.reclaimable();
    for (i, v) in vols.iter().enumerate() {
        add_full(&s, &volume_card(a, v, if i == 0 { safe + review } else { 0 }, true));
    }
    let sc = scroller(mtm, &s);
    frame(a, &tr("Ổ đĩa", "Volumes"), &tr(&format!("{} ổ đang gắn", vols.len()), &format!("{} mounted", vols.len())), vec![], &sc)
}

// ================================================================ history

fn history_page(a: &App) -> Retained<NSView> {
    let mtm = a.mtm;
    let s = page_stack(mtm);
    let records = a.store.recent_actions(300).unwrap_or_default();
    let (c, inner) = card(mtm, 18.0, 0.0);
    if records.is_empty() {
        add(&inner, &label(mtm, &tr("Chưa có thao tác dọn dẹp nào.", "No cleanup actions yet."), &font(13.0, W::Regular), &Palette::secondary()));
    }
    for (i, r) in records.iter().enumerate() {
        if i > 0 {
            add_full(&inner, &separator(mtm));
        }
        let h = hstack(mtm, 12.0);
        h.heightAnchor().constraintEqualToConstant(44.0).setActive(true);
        let when = chrono::DateTime::parse_from_rfc3339(&r.at)
            .map(|t| t.with_timezone(&chrono::Local).format("%d/%m %H:%M").to_string())
            .unwrap_or_else(|_| r.at.clone());
        let tl = label(mtm, &when, &digits(13.0, W::Regular), &Palette::secondary());
        size(&tl, 90.0, 0.0);
        add(&h, &tl);
        let (txt, col) = if r.action == "delete" { (tr("Xoá", "Deleted"), Palette::red()) } else { (tr("Thùng rác", "Trash"), Palette::blue()) };
        let b = badge(mtm, &txt, &col);
        let bw = hstack(mtm, 0.0);
        add(&bw, &b);
        add(&bw, &spacer(mtm));
        size(&bw, 90.0, 0.0);
        add(&h, &bw);
        let pl = label(mtm, &r.path.replace(&*a.home.to_string_lossy(), "~"), &font(13.0, W::Regular), &Palette::primary());
        pl.setLineBreakMode(NSLineBreakMode::ByTruncatingMiddle);
        add(&h, &pl);
        add(&h, &spacer(mtm));
        let ok = r.result == "ok";
        let res = label(mtm, &if ok { tr("Thành công", "Done") } else { r.result.clone() }, &font(12.0, W::Regular), &*if ok { Palette::green() } else { Palette::red() });
        res.setToolTip(Some(&ns(&r.result)));
        add(&h, &res);
        add_full(&inner, &h);
    }
    add_full(&s, &c);
    let sc = scroller(mtm, &s);
    frame(a, &tr("Lịch sử", "History"), &tr(&format!("{} thao tác gần nhất", records.len()), &format!("{} recent actions", records.len())), vec![], &sc)
}

// ================================================================ settings

fn settings_page(a: &App) -> Retained<NSView> {
    let mtm = a.mtm;
    let d = a.draft();
    let text = a.draft_text.borrow().clone().unwrap_or_else(|| {
        [
            (d.min_size_bytes / MB).to_string(),
            d.min_age_days.to_string(),
            d.yellow_gb.to_string(),
            d.red_gb.to_string(),
            d.exclusions.join("\n"),
        ]
    });
    let s = page_stack(mtm);

    // Scanning
    let row1 = columns(mtm, 16.0);
    let (c1, inner) = card(mtm, 18.0, 0.0);
    add(&inner, &label(mtm, &tr("Quét và chạy nền", "Scanning and background"), &font(15.0, W::Semibold), &Palette::primary()));
    inner.setCustomSpacing_afterView(6.0, &inner.arrangedSubviews().lastObject().unwrap());
    type Toggle = (String, Option<String>, bool, fn(&mut Settings, bool));
    let toggles: [Toggle; 4] = [
        (tr("Tự quét mỗi 2 giờ", "Scan every 2 hours"), Some(tr("Chỉ quét và thông báo, không tự xoá", "Scans and notifies only, never deletes")), d.auto_scan, |s, v| s.auto_scan = v),
        (tr("Mở khi đăng nhập", "Open at login"), None, d.open_at_login, |s, v| s.open_at_login = v),
        (tr("Đánh giá bằng Claude CLI", "Review with Claude CLI"), Some(tr("Chỉ gửi đường dẫn, kích thước, ngày", "Sends only paths, sizes and dates")), d.ai_review, |s, v| s.ai_review = v),
        (tr("Quét toàn bộ Home", "Scan all of Home"), Some(tr("Trừ các vùng được bảo vệ", "Except protected areas")), d.scan_home, |s, v| s.scan_home = v),
    ];
    for (i, (title, note, on, set)) in toggles.into_iter().enumerate() {
        if i > 0 {
            add_full(&inner, &separator(mtm));
        }
        let h = hstack(mtm, 8.0);
        h.heightAnchor().constraintEqualToConstant(50.0).setActive(true);
        let t = vstack(mtm, 2.0);
        add(&t, &label(mtm, &title, &font(14.0, W::Regular), &Palette::primary()));
        if let Some(n) = note {
            add(&t, &label(mtm, &n, &font(12.0, W::Regular), &Palette::secondary()));
        }
        add(&h, &t);
        add(&h, &spacer(mtm));
        let sw = switch(mtm, on, &a.target(|_| {}));
        let sw_ref = sw.clone();
        let tgt = a.target(move |a| {
            let v = sw_ref.state() == NSControlStateValueOn;
            a.edit_draft(|s| set(s, v));
        });
        unsafe { sw.setTarget(Some(&tgt)) };
        add(&h, &sw);
        add_full(&inner, &h);
    }
    add(&row1, &c1);

    // Folders
    let (c2, inner) = card(mtm, 18.0, 10.0);
    add(&inner, &label(mtm, &tr("Thư mục quét", "Scan folders"), &font(15.0, W::Semibold), &Palette::primary()));
    let list = boxed(mtm, &Palette::card_alt(), 10.0);
    let ls = vstack(mtm, 0.0);
    if d.roots.is_empty() {
        let e = label(mtm, &tr("Chưa chọn thư mục nào", "No folders selected"), &font(13.0, W::Regular), &Palette::tertiary());
        let w = hstack(mtm, 0.0);
        w.setEdgeInsets(NSEdgeInsets { top: 12.0, left: 12.0, bottom: 12.0, right: 12.0 });
        add(&w, &e);
        add_full(&ls, &w);
    }
    for (i, root) in d.roots.iter().enumerate() {
        if i > 0 {
            add_full(&ls, &separator(mtm));
        }
        let h = hstack(mtm, 10.0);
        h.setEdgeInsets(NSEdgeInsets { top: 0.0, left: 12.0, bottom: 0.0, right: 8.0 });
        h.heightAnchor().constraintEqualToConstant(40.0).setActive(true);
        add(&h, &symbol_view(mtm, "folder.fill", 14.0, &Palette::blue()));
        let l = label(mtm, &root.replace(&*a.home.to_string_lossy(), "~"), &font(14.0, W::Regular), &Palette::primary());
        l.setLineBreakMode(NSLineBreakMode::ByTruncatingMiddle);
        add(&h, &l);
        add(&h, &spacer(mtm));
        let r = root.clone();
        let rm = button(mtm, "", &a.target(move |_| {
            let r = r.clone();
            later(move |a| a.remove_root(&r));
        }));
        rm.setBordered(false);
        with_symbol(&rm, "minus.circle.fill");
        rm.setContentTintColor(Some(&Palette::secondary()));
        add(&h, &rm);
        add_full(&ls, &h);
    }
    list.addSubview(&ls);
    pin(&ls, &list, 0.0);
    add_full(&inner, &list);
    let addb = button(mtm, &tr("Thêm thư mục…", "Add folder…"), &a.target(|_| later(|a| a.pick_root())));
    with_symbol(&addb, "plus");
    add(&inner, &addb);
    add(&inner, &wrapping(mtm, &tr("Thư mục được bảo vệ (Library, iCloud, .git…) sẽ bị từ chối khi thêm.", "Protected folders (Library, iCloud, .git…) are refused."), &font(12.0, W::Regular), &Palette::tertiary()));
    add(&row1, &c2);
    add_full(&s, &row1);

    // Rules + exclusions
    let row2 = columns(mtm, 16.0);
    let (c3, inner) = card(mtm, 18.0, 4.0);
    add(&inner, &label(mtm, &tr("Quy tắc an toàn", "Safety rules"), &font(15.0, W::Semibold), &Palette::primary()));
    let rules = [
        (tr("Kích thước tối thiểu", "Minimum size"), "MB", None),
        (tr("Không sửa ít nhất", "Unmodified for at least"), if crate::app::tr("v", "e") == "v" { "ngày" } else { "days" }, None),
        (tr("Cảnh báo vàng khi trống ≤", "Yellow when free ≤"), "GB", Some(Palette::yellow())),
        (tr("Cảnh báo đỏ khi trống ≤", "Red when free ≤"), "GB", Some(Palette::red())),
    ];
    let mut fields = vec![];
    for (i, (title, unit, color)) in rules.into_iter().enumerate() {
        let h = hstack(mtm, 8.0);
        h.heightAnchor().constraintEqualToConstant(40.0).setActive(true);
        if let Some(c) = color {
            add(&h, &dot(mtm, &c, 8.0));
        }
        add(&h, &label(mtm, &title, &font(14.0, W::Regular), &Palette::primary()));
        add(&h, &spacer(mtm));
        let f = text_field(mtm, &text[i], 80.0);
        add(&h, &f);
        let u = label(mtm, unit, &font(13.0, W::Regular), &Palette::secondary());
        size(&u, 34.0, 0.0);
        add(&h, &u);
        add_full(&inner, &h);
        fields.push(f);
    }
    add(&row2, &c3);
    let (c4, inner) = card(mtm, 18.0, 6.0);
    add(&inner, &label(mtm, &tr("Đường dẫn loại trừ", "Excluded paths"), &font(15.0, W::Semibold), &Palette::primary()));
    add(&inner, &label(mtm, &tr("Mỗi dòng một đường dẫn tuyệt đối", "One absolute path per line"), &font(12.0, W::Regular), &Palette::secondary()));
    let tsv = NSTextView::scrollableTextView(mtm);
    tsv.setTranslatesAutoresizingMaskIntoConstraints(false);
    tsv.heightAnchor().constraintEqualToConstant(132.0).setActive(true);
    tsv.setBorderType(NSBorderType::BezelBorder);
    let tv = tsv.documentView().and_then(|v| v.downcast::<NSTextView>().ok());
    if let Some(tv) = tv.as_ref() {
        tv.setString(&ns(&text[4]));
        tv.setFont(Some(&mono(12.0)));
        tv.setAutomaticQuoteSubstitutionEnabled(false);
        tv.setAutomaticDashSubstitutionEnabled(false);
    }
    add_full(&inner, &tsv);
    add(&row2, &c4);
    add_full(&s, &row2);
    add(&s, &label(mtm, &tr("Vùng hệ thống và dữ liệu ứng dụng luôn được bảo vệ.", "System and app data always stay protected."), &font(12.0, W::Regular), &Palette::tertiary()));
    {
        let mut refs = a.refs.borrow_mut();
        refs.fields = fields;
        refs.exclusions = tv;
    }

    let dirty = a.draft.borrow().as_ref().is_some_and(|d| *d != *a.settings.borrow()) || a.draft_text.borrow().is_some();
    let mut acc: Vec<Retained<NSView>> = vec![];
    if dirty {
        acc.push(label(mtm, &tr("Có thay đổi chưa lưu", "Unsaved changes"), &font(12.0, W::Regular), &Palette::secondary()).into_view());
        acc.push(button(mtm, &tr("Hoàn tác", "Revert"), &a.target(|a| a.revert_settings())).into_view());
    }
    acc.push(primary_button(mtm, &tr("Lưu cài đặt", "Save settings"), &a.target(|a| a.save_settings())).into_view());
    let sc = scroller(mtm, &s);
    frame(a, &tr("Cài đặt", "Settings"), &tr("Vùng hệ thống và dữ liệu ứng dụng luôn được bảo vệ", "System and app data always stay protected"), acc, &sc)
}

// ================================================================ setup

fn setup_page(a: &App) -> Retained<NSView> {
    let mtm = a.mtm;
    let outer = NSView::new(mtm);
    outer.setTranslatesAutoresizingMaskIntoConstraints(false);
    let s = vstack(mtm, 20.0);
    s.setAlignment(NSLayoutAttribute::CenterX);
    outer.addSubview(&s);
    center_in(&s, &outer);
    s.widthAnchor().constraintEqualToConstant(560.0).setActive(true);

    add(&s, &icon_tile(mtm, "checkmark.shield.fill", &Palette::card(), &Palette::green(), 72.0, 34.0));
    let titles = vstack(mtm, 6.0);
    titles.setAlignment(NSLayoutAttribute::CenterX);
    add(&titles, &label(mtm, "Safety Disk Cleaner", &font(26.0, W::Bold), &Palette::primary()));
    add(&titles, &label(mtm, &tr("Gợi ý dọn đĩa an toàn — bạn luôn là người quyết định.", "Safe disk cleanup suggestions — you stay in control."), &font(14.0, W::Regular), &Palette::secondary()));
    add(&s, &titles);

    let (c, inner) = card(mtm, 6.0, 0.0);
    let points = [
        ("magnifyingglass", Palette::blue(), tr("Chỉ quét, không tự xoá", "Scans only, never deletes on its own"), tr("Tìm tệp lớn, lâu không sửa. Mọi thao tác dọn đều do bạn bấm.", "Finds large, old files. Every cleanup is your click.")),
        ("sparkles", Palette::purple(), tr("Claude CLI chỉ xem siêu dữ liệu", "Claude CLI sees metadata only"), tr("Gửi đường dẫn, kích thước, ngày — không bao giờ đọc nội dung tệp.", "Paths, sizes and dates only — never file contents.")),
        ("checkmark.shield.fill", Palette::green(), tr("Vùng hệ thống luôn được bảo vệ", "System areas stay protected"), tr("Library, iCloud, ứng dụng, thư mục .git và tệp ẩn không bao giờ được đề xuất.", "Library, iCloud, apps, .git folders and hidden files are never suggested.")),
    ];
    for (i, (sym, color, title, body)) in points.into_iter().enumerate() {
        if i > 0 {
            add_full(&inner, &separator(mtm));
        }
        let h = hstack(mtm, 14.0);
        h.setAlignment(NSLayoutAttribute::Top);
        h.setEdgeInsets(NSEdgeInsets { top: 14.0, left: 14.0, bottom: 14.0, right: 14.0 });
        add(&h, &symbol_view(mtm, sym, 16.0, &color));
        let t = vstack(mtm, 3.0);
        add(&t, &label(mtm, &title, &font(14.0, W::Semibold), &Palette::primary()));
        let b = wrapping(mtm, &body, &font(13.0, W::Regular), &Palette::secondary());
        add(&t, &b);
        b.widthAnchor().constraintEqualToConstant(470.0).setActive(true);
        add(&h, &t);
        add_full(&inner, &h);
    }
    add_full(&s, &c);

    let choice = columns(mtm, 12.0);
    let home = a.setup_home.get();
    for (is_home, title, note) in [
        (false, tr("Chỉ thư mục Downloads", "Downloads folder only"), tr("Nhanh, an toàn nhất · đề xuất", "Fastest and safest · recommended")),
        (true, tr("Toàn bộ thư mục Home", "Entire Home folder"), tr("Trừ các vùng được bảo vệ", "Except protected areas")),
    ] {
        let on = home == is_home;
        let opt = ClickView::new(mtm, &Palette::tint(&Palette::blue(), 0.16), 12.0);
        let back = boxed(mtm, &Palette::card(), 12.0);
        back.setBorderWidth(2.0);
        back.setBorderColor(&*if on { Palette::blue() } else { Palette::hairline() });
        opt.addSubview(&back);
        pin(&back, &opt, 0.0);
        let t = vstack(mtm, 4.0);
        t.setEdgeInsets(NSEdgeInsets { top: 14.0, left: 16.0, bottom: 14.0, right: 16.0 });
        add(&t, &label(mtm, &title, &font(14.0, W::Semibold), &Palette::primary()));
        add(&t, &label(mtm, &note, &font(12.0, W::Regular), &Palette::secondary()));
        opt.addSubview(&t);
        pin(&t, &opt, 0.0);
        opt.set_selected(on);
        opt.on_click(move || {
            let a = app();
            a.setup_home.set(is_home);
            a.request_render();
        });
        add(&choice, &opt);
    }
    add_full(&s, &choice);
    let go = primary_button(mtm, &tr("Hoàn tất và quét", "Finish and scan"), &a.target(|a| a.finish_setup()));
    go.setKeyEquivalent(&ns("\r"));
    add_full(&s, &go);
    let status = a.status.borrow().clone();
    add(&s, &label(mtm, &if status.is_empty() { tr("Bật tự quét mỗi 2 giờ và mở khi đăng nhập · đổi được trong Cài đặt", "Auto-scan every 2 hours and open at login · change in Settings") } else { status }, &font(12.0, W::Regular), &Palette::tertiary()));
    let _ = ProtocolObject::<dyn NSObjectProtocol>::from_ref(&*outer);
    outer
}
