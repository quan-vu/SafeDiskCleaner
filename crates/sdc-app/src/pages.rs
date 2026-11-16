//! Page builders. Each render clears the main area and builds the current page
//! from `App` state; live values are updated in place by `App::tick`.

use crate::app::*;
use crate::system;
use crate::ui::*;
use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2::{MainThreadMarker, MainThreadOnly, Message};
use objc2_app_kit::*;
use objc2_foundation::*;
use sdc_core::category::Category;
use sdc_core::format;
use sdc_core::overview::{pack, CardPref, CardSize, GRID_COLUMNS};
use sdc_core::model::*;
use sdc_core::snapshot::{compare, Snapshot};
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

    let items: [(Page, &str, Retained<NSColor>, String); 6] = [
        (Page::Overview, "square.grid.2x2.fill", Palette::gray(), tr("Tổng quan", "Overview")),
        (Page::Files, "list.bullet", Palette::green(), tr("Tệp cần xem", "Files to review")),
        (Page::Disk, "waveform.path.ecg", Palette::orange(), tr("Hoạt động đĩa", "Disk activity")),
        (Page::Volumes, "internaldrive.fill", Palette::indigo(), tr("Ổ đĩa", "Volumes")),
        (Page::Storage, "chart.pie.fill", Palette::blue(), tr("Dung lượng thư mục", "Storage")),
        (Page::Changes, "calendar", Palette::cyan(), tr("Thay đổi theo ngày", "Daily changes")),
    ];
    for (page, sym, color, title) in items {
        add_full(&s, &nav_row(a, page, sym, &color, &title));
    }
    add_full(&s, &section(mtm, &tr("Dọn dẹp", "Cleanup")));
    add_full(&s, &nav_row(a, Page::Ignored, "eye.slash.fill", &Palette::gray(), &tr("Đã bỏ qua", "Ignored")));
    add_full(&s, &nav_row(a, Page::History, "clock.fill", &Palette::gray(), &tr("Lịch sử", "History")));
    add_full(&s, &section(mtm, &tr("Danh sách", "Lists")));
    let lists_box = vstack(mtm, 2.0);
    add_full(&s, &lists_box);
    *a.lists_box.borrow_mut() = Some(lists_box);
    rebuild_sidebar_lists(a);
    let new_list = link_button(mtm, &tr("Danh sách mới", "New list"), &Palette::blue(), &a.keep_target(|_| later(|a| a.new_list(false))));
    with_symbol(&new_list, "plus");
    let nl = hstack(mtm, 0.0);
    nl.setEdgeInsets(NSEdgeInsets { top: 4.0, left: 10.0, bottom: 0.0, right: 10.0 });
    add(&nl, &new_list);
    add_full(&s, &nl);
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

/// Refill the sidebar's list rows from `a.lists`.
pub fn rebuild_sidebar_lists(a: &App) {
    let Some(stack) = a.lists_box.borrow().clone() else { return };
    a.nav.borrow_mut().retain(|n| !matches!(n.page, Page::List(_)));
    for v in stack.arrangedSubviews().iter() {
        stack.removeArrangedSubview(&v);
        v.removeFromSuperview();
    }
    let lists = a.lists.borrow().clone();
    for l in &lists {
        let row = nav_row(a, Page::List(l.id), "bookmark.fill", &Palette::orange(), &l.name);
        add_full(&stack, &row);
    }
    a.highlight_nav();
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
    t.setLineBreakMode(NSLineBreakMode::ByTruncatingTail);
    t.setContentCompressionResistancePriority_forOrientation(250.0, NSLayoutConstraintOrientation::Horizontal);
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
    let mut lines = vec![];
    for id in ["temp", "cpu", "ram"] {
        let mi = unsafe { NSMenuItem::initWithTitle_action_keyEquivalent(NSMenuItem::alloc(mtm), &ns(&a.tray_line(id)), None, &ns("")) };
        mi.setEnabled(false);
        menu.addItem(&mi);
        lines.push((id, mi));
    }
    *a.tray_lines.borrow_mut() = lines;
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
    action(&tr("Quét ngay", "Scan now"), "r", |a| a.scan_now());
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
        Page::Files => files_page(a, Mode::Review),
        Page::Ignored => files_page(a, Mode::Ignored),
        Page::Changes => changes_page(a),
        Page::Storage => storage_page(a),
        Page::List(id) => {
            if !a.lists.borrow().iter().any(|l| l.id == id) {
                a.page.set(Page::Files);
                a.highlight_nav();
                files_page(a, Mode::Review)
            } else {
                // Prefer the latest scan data for files still in the scan results.
                let current = a.candidates.borrow();
                let items = a
                    .store
                    .bookmarks(id)
                    .unwrap_or_default()
                    .into_iter()
                    .map(|b| current.iter().find(|c| c.id == b.id).cloned().map(|mut c| { c.ignored = false; c }).unwrap_or(b))
                    .collect();
                drop(current);
                *a.list_items.borrow_mut() = items;
                files_page(a, Mode::List(id))
            }
        }
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
        let show = button(mtm, &tr("Xem tiến trình", "Show progress"), &a.target(|_| later(|a| a.open_scan_sheet())));
        add(&h, &show);
        let stop = button(mtm, &tr("Dừng", "Stop"), &a.target(|a| a.stop_scan()));
        add(&h, &stop);
        return h.into_view();
    }
    let b = primary_button(mtm, &tr("Quét ngay", "Scan now"), &a.target(|_| later(|a| a.scan_now())));
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

const CARD_ROW_H: f64 = 190.0;

fn overview_page(a: &App) -> Retained<NSView> {
    let mtm = a.mtm;
    let s = page_stack(mtm);
    let cards = a.cards();
    let sizes: Vec<CardSize> = cards.iter().map(|c| c.size).collect();
    let (places, rows) = pack(&sizes, GRID_COLUMNS);
    let grid = CardGrid::new(mtm, GRID_COLUMNS, CARD_ROW_H, 16.0);
    for (i, (c, p)) in cards.iter().zip(places).enumerate() {
        let v = overview_card(a, c, i, cards.len());
        grid.put(&v, p.col, p.row, p.cols, p.rows);
    }
    grid.set_rows(rows);
    add_full(&s, &grid);

    let mut acc: Vec<Retained<NSView>> = vec![];
    if a.arranging.get() {
        acc.push(button(mtm, &tr("Đặt lại", "Reset layout"), &a.target(|a| a.reset_cards())).into_view());
        let done = primary_button(mtm, &tr("Xong", "Done"), &a.target(|a| {
            a.arranging.set(false);
            a.request_render();
        }));
        acc.push(done.into_view());
    } else {
        let arrange = button(mtm, &tr("Sắp xếp thẻ", "Arrange cards"), &a.target(|a| {
            a.arranging.set(true);
            a.request_render();
        }));
        with_symbol(&arrange, "square.grid.2x2");
        acc.push(arrange.into_view());
        acc.push(scan_button(a));
    }
    let subtitle = if a.arranging.get() {
        tr("Chọn cỡ S, M, L cho từng thẻ và di chuyển bằng mũi tên", "Choose a size (S, M, L) for each card and move cards with the arrows")
    } else {
        format!("{} {}", tr("Lần quét gần nhất:", "Last scan:"), fmt_time(&a.settings.borrow().last_scan))
    };
    let sc = scroller(mtm, &s);
    frame(a, &tr("Tổng quan", "Overview"), &subtitle, acc, &sc)
}

/// Card shell for the overview grid: content pinned to the top, clipped to the cell.
fn overview_card(a: &App, c: &CardPref, index: usize, count: usize) -> Retained<NSView> {
    let mtm = a.mtm;
    let b = boxed(mtm, &Palette::card(), 14.0);
    if a.arranging.get() {
        b.setBorderColor(&Palette::tint(&Palette::blue(), 0.6));
        b.setBorderWidth(1.5);
    }
    b.setWantsLayer(true);
    if let Some(layer) = b.layer() {
        layer.setMasksToBounds(true);
    }
    let st = vstack(mtm, 10.0);
    st.setEdgeInsets(NSEdgeInsets { top: 16.0, left: 18.0, bottom: 16.0, right: 18.0 });
    b.addSubview(&st);
    st.topAnchor().constraintEqualToAnchor(&b.topAnchor()).setActive(true);
    st.leadingAnchor().constraintEqualToAnchor(&b.leadingAnchor()).setActive(true);
    st.trailingAnchor().constraintEqualToAnchor(&b.trailingAnchor()).setActive(true);
    st.bottomAnchor().constraintLessThanOrEqualToAnchor(&b.bottomAnchor()).setActive(true);
    if a.arranging.get() {
        add_full(&st, &arrange_bar(a, c, index, count));
    }
    match c.id.as_str() {
        "disk" => disk_card(a, &st, c.size),
        "temp" => temp_card(a, &st, c.size),
        "cpu" => cpu_card(a, &st, c.size),
        _ => ram_card(a, &st, c.size),
    }
    b.into_view()
}

fn arrange_bar(a: &App, c: &CardPref, index: usize, count: usize) -> Retained<NSStackView> {
    let mtm = a.mtm;
    let h = hstack(mtm, 6.0);
    let sel = match c.size {
        CardSize::S => 0,
        CardSize::M => 1,
        CardSize::L => 2,
    };
    let seg = segmented(mtm, &["S", "M", "L"], sel, &a.target(|_| {}));
    let seg_ref = seg.clone();
    let id = c.id.clone();
    let t = a.target(move |a| {
        let size = match seg_ref.selectedSegment() {
            0 => CardSize::S,
            1 => CardSize::M,
            _ => CardSize::L,
        };
        let id = id.clone();
        later(move |a| a.set_card_size(&id, size));
        let _ = a;
    });
    unsafe { seg.setTarget(Some(&t)) };
    seg.setToolTip(Some(&ns(&tr("Cỡ thẻ", "Card size"))));
    add(&h, &seg);
    add(&h, &spacer(mtm));
    for (sym, step, enabled, tip) in [
        ("chevron.left", -1isize, index > 0, tr("Đưa lên trước", "Move earlier")),
        ("chevron.right", 1isize, index + 1 < count, tr("Đưa ra sau", "Move later")),
    ] {
        let id = c.id.clone();
        let b = button(mtm, "", &a.target(move |_| {
            let id = id.clone();
            later(move |a| a.move_card(&id, step));
        }));
        with_symbol(&b, sym);
        b.setImagePosition(NSCellImagePosition::ImageOnly);
        b.setEnabled(enabled);
        b.setToolTip(Some(&ns(&tip)));
        b.setAccessibilityLabel(Some(&ns(&tip)));
        add(&h, &b);
    }
    h
}

/// Icon tile + title (+ optional trailing view) for an overview card.
fn card_title(mtm: MainThreadMarker, sym: &str, color: &NSColor, title: &str, sub: Option<&str>, trailing: Option<Retained<NSView>>) -> Retained<NSStackView> {
    let h = hstack(mtm, 10.0);
    add(&h, &icon_tile(mtm, sym, &Palette::tint(color, 0.18), color, 28.0, 13.0));
    let t = vstack(mtm, 1.0);
    add(&t, &label(mtm, title, &font(14.0, W::Semibold), &Palette::primary()));
    if let Some(sub) = sub {
        add(&t, &label(mtm, sub, &font(12.0, W::Regular), &Palette::secondary()));
    }
    add(&h, &t);
    add(&h, &spacer(mtm));
    if let Some(v) = trailing {
        add(&h, &v);
    }
    h
}

/// "name ........ value" row with a hairline under it.
fn value_row(mtm: MainThreadMarker, dot_color: Option<&NSColor>, name: &str, value: &str, note: Option<&str>) -> (Retained<NSStackView>, Retained<NSTextField>) {
    let h = hstack(mtm, 10.0);
    h.heightAnchor().constraintEqualToConstant(34.0).setActive(true);
    if let Some(c) = dot_color {
        add(&h, &dot(mtm, c, 8.0));
    }
    let n = label(mtm, name, &font(13.0, W::Regular), &Palette::primary());
    n.setLineBreakMode(NSLineBreakMode::ByTruncatingMiddle);
    n.setContentCompressionResistancePriority_forOrientation(100.0, NSLayoutConstraintOrientation::Horizontal);
    add(&h, &n);
    add(&h, &spacer(mtm));
    let v = label(mtm, value, &digits(13.0, W::Semibold), &Palette::primary());
    add(&h, &v);
    if let Some(note) = note {
        let l = label(mtm, note, &font(12.0, W::Regular), &Palette::secondary());
        l.setAlignment(NSTextAlignment::Right);
        size(&l, 64.0, 0.0);
        add(&h, &l);
    }
    (h, v)
}

fn hero_size(size: CardSize, big: bool) -> f64 {
    match (size, big) {
        (CardSize::S, true) => 34.0,
        (CardSize::M, true) => 44.0,
        (CardSize::L, true) => 56.0,
        (CardSize::S, false) => 28.0,
        (CardSize::M, false) => 30.0,
        (CardSize::L, false) => 34.0,
    }
}

fn caption(mtm: MainThreadMarker, text: &str) -> Retained<NSTextField> {
    label(mtm, text, &font(12.0, W::Medium), &Palette::secondary())
}

fn disk_card(a: &App, st: &NSStackView, size: CardSize) {
    let mtm = a.mtm;
    let Some(v) = a.volumes.borrow().first().cloned() else {
        add_full(st, &card_title(mtm, "internaldrive.fill", &Palette::indigo(), &tr("Ổ đĩa", "Disk"), None, None));
        add(st, &label(mtm, &tr("Chưa đọc được thông tin ổ đĩa.", "Disk information is unavailable."), &font(13.0, W::Regular), &Palette::secondary()));
        return;
    };
    let (safe, n_safe, review, n_review) = a.reclaimable();
    let sub = format!("{} · {}", v.fs_type, v.bsd_name);
    let pill = badge(mtm, &level_text(v.level), &level_color(v.level)).into_view();
    add_full(st, &card_title(mtm, "internaldrive.fill", &Palette::indigo(), &v.name, (size != CardSize::S).then_some(sub.as_str()), Some(pill)));
    let g = vstack(mtm, 2.0);
    add(&g, &caption(mtm, &tr("Dung lượng trống", "Free space")));
    let (n, u) = format::bytes_parts(v.free_bytes);
    add(&g, &hero_total(mtm, &n, &u, &format!("/ {}", format::bytes(v.total_bytes)), hero_size(size, true)));
    add_full(st, &g);
    let total = v.total_bytes.max(1) as f64;
    let used = (v.total_bytes - v.free_bytes) as f64 / total;
    let rec = ((safe + review) as f64 / total).min(used);
    add_full(st, &Bar::new(mtm, 10.0, vec![(0.0, used - rec, Palette::indigo()), (used - rec, used, Palette::orange())], 2.0));
    if size != CardSize::S {
        let lg = hstack(mtm, 16.0);
        add(&lg, &legend(mtm, &Palette::indigo(), &format!("{} {}", tr("Đã dùng", "Used"), format::bytes(v.total_bytes - v.free_bytes - safe - review))));
        add(&lg, &legend(mtm, &Palette::orange(), &format!("{} {}", tr("Có thể dọn", "Reclaimable"), format::bytes(safe + review))));
        add(&lg, &legend(mtm, &Palette::track(), &format!("{} {}", tr("Trống", "Free"), format::bytes(v.free_bytes))));
        add(st, &lg);
    }
    if size == CardSize::L {
        add_full(st, &separator(mtm));
        let files = |n: usize| tr(&format!("{n} tệp"), &format!("{n} files"));
        add_full(st, &value_row(mtm, Some(&Palette::green()), &tr("An toàn để dọn", "Safe to clean"), &format::bytes(safe), Some(&files(n_safe))).0);
        add_full(st, &value_row(mtm, Some(&Palette::yellow()), &tr("Cần xem lại", "Needs review"), &format::bytes(review), Some(&files(n_review))).0);
        let foot = hstack(mtm, 8.0);
        let s = a.settings.borrow();
        let next = if s.auto_scan { format!(" · {} {}", tr("tự quét mỗi", "auto-scan every"), interval_text(s.scan_interval_minutes)) } else { String::new() };
        add(&foot, &label(mtm, &format!("{} {}{next}", tr("Lần quét:", "Last scan:"), fmt_time(&s.last_scan)), &font(12.0, W::Regular), &Palette::secondary()));
        drop(s);
        add(&foot, &spacer(mtm));
        add(&foot, &link_button(mtm, &tr("Xem tệp →", "Review files →"), &Palette::blue(), &a.target(|a| a.navigate(Page::Files))));
        add_full(st, &foot);
    }
}

fn temp_card(a: &App, st: &NSStackView, size: CardSize) {
    let mtm = a.mtm;
    let (_, _, _, temp) = a.live_metrics();
    add_full(st, &card_title(mtm, "clock.arrow.circlepath", &Palette::orange(), &tr("Tệp tạm", "Temporary"), None, None));
    let Some(t) = temp else {
        add(st, &hero_total(mtm, "…", "", "", hero_size(size, false)));
        add(st, &caption(mtm, &tr("Đang đo bộ nhớ đệm, nhật ký, Thùng rác…", "Measuring caches, logs and Trash…")));
        return;
    };
    let g = vstack(mtm, 2.0);
    let total = format::bytes(t.total());
    let (n, u) = total.rsplit_once(' ').unwrap_or((&total, ""));
    add(&g, &hero_total(mtm, n, u, "", hero_size(size, false)));
    add(&g, &caption(mtm, &tr("bộ nhớ đệm, nhật ký, Thùng rác", "caches, logs, Trash")));
    add_full(st, &g);
    if size == CardSize::S {
        return;
    }
    let sum = t.total().max(1) as f64;
    let (c, l) = (t.caches as f64 / sum, t.logs as f64 / sum);
    let light = Palette::tint(&Palette::orange(), 0.45);
    add_full(st, &Bar::new(mtm, 8.0, vec![(0.0, c, Palette::orange()), (c, c + l, Palette::brown()), (c + l, 1.0, light.clone())], 2.0));
    if size == CardSize::M {
        let lg = hstack(mtm, 14.0);
        add(&lg, &legend(mtm, &Palette::orange(), &format!("{} {}", tr("Đệm", "Caches"), format::bytes(t.caches))));
        add(&lg, &legend(mtm, &Palette::brown(), &format!("{} {}", tr("Nhật ký", "Logs"), format::bytes(t.logs))));
        add(&lg, &legend(mtm, &light, &format!("{} {}", tr("Thùng rác", "Trash"), format::bytes(t.trash))));
        add(st, &lg);
        return;
    }
    add_full(st, &separator(mtm));
    let home = a.home.clone();
    for (color, name, path, bytes) in [
        (Palette::orange(), "~/Library/Caches".to_string(), home.join("Library/Caches"), t.caches),
        (Palette::brown(), "~/Library/Logs".to_string(), home.join("Library/Logs"), t.logs),
        (light, tr("Thùng rác", "Trash"), home.join(".Trash"), t.trash),
    ] {
        let (row, _) = value_row(mtm, Some(&color), &name, &format::bytes(bytes), None);
        let p = path.to_string_lossy().into_owned();
        let reveal = link_button(mtm, &tr("Mở", "Reveal"), &Palette::blue(), &a.target(move |_| system::reveal(std::slice::from_ref(&p))));
        add(&row, &reveal);
        add_full(st, &row);
    }
    add(st, &wrapping(mtm, &tr("Ứng dụng tự tạo lại bộ nhớ đệm khi cần. Không có gì ở đây bị xoá tự động.", "Apps rebuild caches as needed. Nothing here is removed automatically."), &font(12.0, W::Regular), &Palette::tertiary()));
}

fn cpu_card(a: &App, st: &NSStackView, size: CardSize) {
    let mtm = a.mtm;
    let (cpu, cores, _, _) = a.live_metrics();
    add_full(st, &card_title(mtm, "cpu", &Palette::cyan(), "CPU", None, None));
    let g = vstack(mtm, 2.0);
    let hs = hero_size(size, false);
    let now = cpu.last().copied().unwrap_or(0.0);
    let hero = hero_total(mtm, &format!("{now:.0}"), "%", "", hs);
    add(&g, &hero);
    add(&g, &caption(mtm, &tr(&format!("trên {cores} nhân"), &format!("of {cores} cores"))));
    add_full(st, &g);
    let mut refs_spark = None;
    let mut stats = vec![];
    if size != CardSize::S {
        let sp = Spark::new(mtm, if size == CardSize::L { 120.0 } else { 46.0 }, 100.0, &Palette::cyan());
        sp.set_values(cpu.clone());
        add_full(st, &sp);
        refs_spark = Some(sp);
    }
    if size == CardSize::L {
        add_full(st, &separator(mtm));
        let (r1, v1) = value_row(mtm, None, &tr("Trung bình 2 phút", "Average, last 2 min"), "—", None);
        let (r2, v2) = value_row(mtm, None, &tr("Cao nhất 2 phút", "Peak, last 2 min"), "—", None);
        add_full(st, &r1);
        add_full(st, &r2);
        stats = vec![v1, v2];
    }
    let mut refs = a.refs.borrow_mut();
    refs.cpu_value = Some((hero, hs));
    refs.cpu_spark = refs_spark;
    refs.cpu_stats = stats;
}

fn ram_card(a: &App, st: &NSStackView, size: CardSize) {
    let mtm = a.mtm;
    let (_, _, mem, _) = a.live_metrics();
    let m = mem.unwrap_or_default();
    let ratio = m.used() as f64 / m.total.max(1) as f64;
    let (ptext, pcolor) = if ratio < 0.8 {
        (tr("Bình thường", "Normal"), Palette::green())
    } else if ratio < 0.92 {
        (tr("Khá cao", "Elevated"), Palette::yellow())
    } else {
        (tr("Cao", "High"), Palette::red())
    };
    let pill = (size != CardSize::S && mem.is_some()).then(|| badge(mtm, &ptext, &pcolor).into_view());
    add_full(st, &card_title(mtm, "memorychip", &Palette::blue(), &tr("Bộ nhớ", "Memory"), None, pill));
    let hs = hero_size(size, false);
    let used = gib(m.used());
    let (n, u) = used.rsplit_once(' ').unwrap_or((&used, ""));
    let hero = hero_total(mtm, n, u, &format!("/ {}", gib(m.total)), hs);
    add(st, &hero);
    let mut bar_ref = None;
    let mut labels = vec![];
    if size != CardSize::S {
        let t = m.total.max(1) as f64;
        let bar = Bar::new(mtm, 8.0, mem_segments(m.app as f64 / t, m.wired as f64 / t, m.compressed as f64 / t), 2.0);
        add_full(st, &bar);
        bar_ref = Some(bar);
    }
    let light = Palette::tint(&Palette::blue(), 0.45);
    if size == CardSize::M {
        let lg = hstack(mtm, 14.0);
        for (part, color) in [(0u8, Palette::blue()), (1, Palette::indigo()), (2, light.clone())] {
            let row = hstack(mtm, 6.0);
            add(&row, &dot(mtm, &color, 8.0));
            let l = label(mtm, "", &font(12.0, W::Regular), &Palette::secondary());
            add(&row, &l);
            add(&lg, &row);
            labels.push((part, l, true));
        }
        add(st, &lg);
    }
    if size == CardSize::L {
        add_full(st, &separator(mtm));
        for (part, color, name) in [
            (0u8, Some(Palette::blue()), tr("Bộ nhớ ứng dụng", "App memory")),
            (1, Some(Palette::indigo()), tr("Cố định (hệ thống)", "Wired (system)")),
            (2, Some(light.clone()), tr("Đã nén", "Compressed")),
            (3, None, tr("Còn trống", "Available")),
        ] {
            let (row, v) = value_row(mtm, color.as_deref(), &name, "", None);
            add_full(st, &row);
            labels.push((part, v, false));
        }
    }
    {
        let mut refs = a.refs.borrow_mut();
        refs.mem_value = Some((hero, hs));
        refs.mem_bar = bar_ref;
        refs.mem_labels = labels;
    }
    a.update_live_cards();
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

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Review,
    Ignored,
    List(i64),
}

/// Pop-up of file categories ("All types" first) bound to `a.category_filter`.
fn category_popup(a: &App) -> Retained<NSView> {
    let mtm = a.mtm;
    let pop = unsafe {
        NSPopUpButton::initWithFrame_pullsDown(NSPopUpButton::alloc(mtm), NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(130.0, 26.0)), false)
    };
    pop.addItemWithTitle(&ns(&tr("Mọi loại", "All types")));
    for c in Category::ALL {
        let (vi, en) = c.names();
        pop.addItemWithTitle(&ns(&tr(vi, en)));
    }
    let current = a.category_filter.get().and_then(|c| Category::ALL.iter().position(|x| *x == c)).map(|i| i + 1).unwrap_or(0);
    pop.selectItemAtIndex(current as isize);
    let pop_ref = pop.clone();
    let t = a.target(move |a| {
        let i = pop_ref.indexOfSelectedItem();
        a.category_filter.set(if i <= 0 { None } else { Category::ALL.get(i as usize - 1).copied() });
        a.request_render();
    });
    unsafe {
        pop.setTarget(Some(&t));
        pop.setAction(Some(fire_sel()));
    }
    pop.into_view()
}

/// "Add to list" button that pops up the lists plus "New list…".
fn add_to_list_button(a: &App) -> Retained<NSButton> {
    let mtm = a.mtm;
    let b = button(mtm, &tr("Thêm vào danh sách", "Add to list"), &a.target(|_| {}));
    with_symbol(&b, "bookmark");
    let b_ref = b.clone();
    let t = a.target(move |a| {
        let menu = NSMenu::new(a.mtm);
        menu.setAutoenablesItems(false);
        for l in a.lists.borrow().iter() {
            let id = l.id;
            let item = unsafe {
                NSMenuItem::initWithTitle_action_keyEquivalent(NSMenuItem::alloc(a.mtm), &ns(&l.name), Some(fire_sel()), &ns(""))
            };
            let tg = a.target(move |_| later(move |a| a.add_selected_to_list(id)));
            unsafe { item.setTarget(Some(&tg)) };
            menu.addItem(&item);
        }
        if !a.lists.borrow().is_empty() {
            menu.addItem(&NSMenuItem::separatorItem(a.mtm));
        }
        let item = unsafe {
            NSMenuItem::initWithTitle_action_keyEquivalent(NSMenuItem::alloc(a.mtm), &ns(&tr("Danh sách mới…", "New list…")), Some(fire_sel()), &ns(""))
        };
        let tg = a.target(|_| later(|a| a.new_list(true)));
        unsafe { item.setTarget(Some(&tg)) };
        menu.addItem(&item);
        let h = b_ref.bounds().size.height;
        menu.popUpMenuPositioningItem_atLocation_inView(None, NSPoint::new(0.0, h + 4.0), Some(&b_ref));
    });
    unsafe { b.setTarget(Some(&t)) };
    b
}

fn files_page(a: &App, mode: Mode) -> Retained<NSView> {
    let mtm = a.mtm;
    let ignored_page = mode == Mode::Ignored;
    let query = a.filter.borrow().to_lowercase();
    let tab = a.tab.get();
    let category = a.category_filter.get();
    let source: Vec<Candidate> = match mode {
        Mode::List(_) => a.list_items.borrow().clone(),
        _ => a.candidates.borrow().iter().filter(|c| c.ignored == ignored_page).cloned().collect(),
    };
    let mut items: Vec<Candidate> = source
        .into_iter()
        .filter(|c| query.is_empty() || c.path.to_lowercase().contains(&query))
        .filter(|c| mode != Mode::Review || tab == 0 || (tab == 1) == (c.safety == Safety::Safe))
        .filter(|c| category.is_none_or(|k| Category::of(Path::new(&c.path)) == k))
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
        let (sym, text) = match mode {
            Mode::Ignored => ("eye.slash", tr("Không có tệp nào bị bỏ qua.", "No ignored files.")),
            Mode::List(_) => (
                "bookmark",
                tr("Danh sách trống. Chọn tệp ở \"Tệp cần xem\" rồi bấm \"Thêm vào danh sách\".", "This list is empty. Select files in Files to review and choose Add to list."),
            ),
            Mode::Review => ("checkmark.circle", tr("Không có tệp phù hợp.", "No matching files.")),
        };
        add(&empty, &symbol_view(mtm, sym, 28.0, &Palette::tertiary()));
        add(&empty, &label(mtm, &text, &font(14.0, W::Regular), &Palette::secondary()));
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
    let mut buttons = vec![reveal];
    match mode {
        Mode::List(_) => {
            buttons.push(button(mtm, &tr("Bỏ khỏi danh sách", "Remove from list"), &a.target(|a| a.remove_selected_from_list())));
        }
        Mode::Ignored => {
            buttons.push(add_to_list_button(a));
            buttons.push(button(mtm, &tr("Đưa lại danh sách", "Restore"), &a.target(|a| a.ignore_selected(false))));
        }
        Mode::Review => {
            buttons.push(add_to_list_button(a));
            buttons.push(button(mtm, &tr("Bỏ qua", "Ignore"), &a.target(|a| a.ignore_selected(true))));
        }
    }
    buttons.push(destructive_button(mtm, &tr("Xoá vĩnh viễn…", "Delete permanently…"), &a.target(|_| later(|a| a.confirm_delete()))));
    let trash = primary_button(mtm, &tr("Chuyển vào Thùng rác", "Move to Trash"), &a.target(|_| later(|a| a.confirm_trash())));
    with_symbol(&trash, "trash");
    buttons.push(trash);
    for b in &buttons {
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
        refs.sel_buttons = buttons;
    }

    // Header accessories: search + filters
    let search = NSSearchField::new(mtm);
    search.setPlaceholderString(Some(&ns(&tr("Lọc theo đường dẫn", "Filter by path"))));
    search.setStringValue(&ns(&a.filter.borrow()));
    search.setTranslatesAutoresizingMaskIntoConstraints(false);
    search.widthAnchor().constraintEqualToConstant(200.0).setActive(true);
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
    let mut acc: Vec<Retained<NSView>> = vec![search.into_view(), category_popup(a)];
    let (safe_n, review_n) = {
        let c = a.candidates.borrow();
        (
            c.iter().filter(|c| !c.ignored && c.safety == Safety::Safe).count(),
            c.iter().filter(|c| !c.ignored && c.safety != Safety::Safe).count(),
        )
    };
    if mode == Mode::Review {
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
    let (title, subtitle) = match mode {
        Mode::Ignored => (tr("Đã bỏ qua", "Ignored"), tr(&format!("{} tệp không còn được gợi ý", items.len()), &format!("{} files no longer suggested", items.len()))),
        Mode::Review => (
            tr("Tệp cần xem", "Files to review"),
            tr(&format!("{} tệp · tối đa {}", items.len(), format::bytes(total)), &format!("{} files · up to {}", items.len(), format::bytes(total))),
        ),
        Mode::List(id) => {
            let rename = button(mtm, &tr("Đổi tên", "Rename"), &a.target(move |_| later(move |a| a.rename_list(id))));
            with_symbol(&rename, "pencil");
            acc.push(rename.into_view());
            let del = button(mtm, &tr("Xoá danh sách", "Delete list"), &a.target(move |_| later(move |a| a.delete_list(id))));
            with_symbol(&del, "trash");
            acc.push(del.into_view());
            let name = a.lists.borrow().iter().find(|l| l.id == id).map(|l| l.name.clone()).unwrap_or_default();
            (name, tr(&format!("{} tệp · {}", items.len(), format::bytes(total)), &format!("{} files · {}", items.len(), format::bytes(total))))
        }
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
    let name_line = hstack(mtm, 5.0);
    let nl = label(mtm, &p.file_name().unwrap_or_default().to_string_lossy(), &font(14.0, W::Semibold), &Palette::primary());
    nl.setLineBreakMode(NSLineBreakMode::ByTruncatingMiddle);
    nl.setContentCompressionResistancePriority_forOrientation(100.0, NSLayoutConstraintOrientation::Horizontal);
    add(&name_line, &nl);
    if let Some(lists) = a.bookmark_index.borrow().get(&c.id) {
        let mark = symbol_view(mtm, "bookmark.fill", 11.0, &Palette::orange());
        mark.setToolTip(Some(&ns(&lists.join(", "))));
        add(&name_line, &mark);
    }
    add(&name_col, &name_line);
    let dir_line = hstack(mtm, 4.0);
    let cat = Category::of(p);
    let (vi, en) = cat.names();
    add(&dir_line, &symbol_view(mtm, cat.symbol(), 10.0, &Palette::secondary()));
    add(&dir_line, &label(mtm, &format!("{} ·", tr(vi, en)), &font(12.0, W::Medium), &Palette::secondary()));
    let dir = p.parent().map(|d| d.to_string_lossy().replace(&*a.home.to_string_lossy(), "~")).unwrap_or_default();
    let dl = label(mtm, &dir, &font(12.0, W::Regular), &Palette::tertiary());
    dl.setLineBreakMode(NSLineBreakMode::ByTruncatingMiddle);
    dl.setContentCompressionResistancePriority_forOrientation(100.0, NSLayoutConstraintOrientation::Horizontal);
    add(&dir_line, &dl);
    add(&name_col, &dir_line);
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

// ================================================================ storage

/// Display name for a usage part (computed parts are translated, folder names kept).
fn part_label(label: &str) -> String {
    match label {
        "Home" => tr("Thư mục Home", "Home"),
        "Applications" => tr("Ứng dụng", "Applications"),
        "Library" => tr("Thư viện hệ thống", "Library"),
        "System & other" => tr("Hệ thống và khác", "System & other"),
        "Free" => tr("Còn trống", "Free"),
        "Other" => tr("Khác", "Other"),
        "Files in Home" => tr("Tệp lẻ trong Home", "Files in Home"),
        _ => label.to_string(),
    }
}

fn storage_page(a: &App) -> Retained<NSView> {
    let mtm = a.mtm;
    let s = page_stack(mtm);
    let scope = a.usage_scope.get();
    let pie = a.usage_chart.get() == 0;
    let usage = if scope == 0 { a.usage_home.borrow().clone() } else { a.usage_disk.borrow().clone() };

    // Header controls: scope, chart form, measure.
    let mut acc: Vec<Retained<NSView>> = vec![];
    let scope_seg = segmented(mtm, &[&tr("Home", "Home"), &tr("Ổ đĩa", "Disk")], scope, &a.target(|_| {}));
    let sr = scope_seg.clone();
    let t = a.target(move |a| {
        a.usage_scope.set(sr.selectedSegment());
        a.request_render();
    });
    unsafe { scope_seg.setTarget(Some(&t)) };
    acc.push(scope_seg.into_view());
    let chart_seg = segmented(mtm, &[&tr("Tròn", "Pie"), &tr("Cột", "Columns")], a.usage_chart.get(), &a.target(|_| {}));
    let cr = chart_seg.clone();
    let t = a.target(move |a| {
        a.usage_chart.set(cr.selectedSegment());
        a.request_render();
    });
    unsafe { chart_seg.setTarget(Some(&t)) };
    acc.push(chart_seg.into_view());
    let status = label(mtm, "", &font(12.0, W::Regular), &Palette::secondary());
    status.setLineBreakMode(NSLineBreakMode::ByTruncatingMiddle);
    if a.usage_measuring.get() {
        let h = hstack(mtm, 8.0);
        let spin = NSProgressIndicator::new(mtm);
        spin.setStyle(NSProgressIndicatorStyle::Spinning);
        spin.setControlSize(NSControlSize::Small);
        unsafe { spin.startAnimation(None) };
        add(&h, &spin);
        add(&h, &button(mtm, &tr("Dừng", "Stop"), &a.target(|a| a.stop_usage())));
        acc.push(h.into_view());
    } else {
        let b = primary_button(mtm, &tr("Đo lại", "Measure again"), &a.target(|a| {
            a.measure_usage();
            a.request_render();
        }));
        with_symbol(&b, "arrow.clockwise");
        acc.push(b.into_view());
    }

    let (c, inner) = card(mtm, 20.0, 12.0);
    match &usage {
        None => {
            add(&inner, &label(mtm, &tr("Chưa có số liệu.", "No measurements yet."), &font(15.0, W::Semibold), &Palette::primary()));
            add(&inner, &wrapping(mtm, &tr(
                "Đo dung lượng từng thư mục trong Home có thể mất vài phút. Kết quả được lưu lại và tự đo lại sau 6 giờ khi bạn mở trang này.",
                "Measuring every folder in Home can take a few minutes. Results are saved and re-measured after 6 hours when you open this page.",
            ), &font(13.0, W::Regular), &Palette::secondary()));
            if a.usage_measuring.get() {
                add_full(&inner, &status);
            }
        }
        Some(u) => {
            let total_all = u.total().max(1);
            let parts = if scope == 0 {
                sdc_core::usage::fold(&u.parts, 8, &[])
            } else {
                sdc_core::usage::fold(&u.parts, 8, &["Free"])
            };
            let title = if scope == 0 {
                format!("{} · {}", tr("Thư mục Home", "Home folder"), format::bytes(u.total()))
            } else {
                let name = a.volumes.borrow().first().map(|v| v.name.clone()).unwrap_or_else(|| "Disk".into());
                format!("{name} · {}", format::bytes(u.total()))
            };
            let sub = if scope == 0 {
                tr("Dung lượng từng thư mục ở cấp đầu của Home", "Space used by each top-level folder in Home")
            } else {
                tr("Dung lượng đã dùng chia theo khu vực, cộng phần còn trống", "Used space split by area, plus free space")
            };
            add_full(&inner, &card_title(mtm, "chart.pie.fill", &Palette::blue(), &title, Some(&sub), None));
            if a.usage_measuring.get() {
                add_full(&inner, &status);
            }
            // Colors follow the part: slot by position among named parts, neutral for Other/Free.
            let mut slot = 0;
            let colors: Vec<Retained<NSColor>> = parts
                .iter()
                .map(|p| match p.label.as_str() {
                    "Other" => other_color(),
                    "Free" => Palette::track(),
                    _ => {
                        let c = series_color(slot);
                        slot += 1;
                        c
                    }
                })
                .collect();
            let legend = vstack(mtm, 0.0);
            for (i, (p, col)) in parts.iter().zip(&colors).enumerate() {
                if i > 0 {
                    add_full(&legend, &separator(mtm));
                }
                let pct = p.bytes as f64 * 100.0 / total_all as f64;
                let (row, _) = value_row(mtm, if pie { Some(&**col) } else { None }, &part_label(&p.label), &format::bytes(p.bytes), Some(&format!("{pct:.1}%")));
                if !p.path.is_empty() {
                    let path = p.path.clone();
                    let rv = link_button(mtm, &tr("Mở", "Reveal"), &Palette::blue(), &a.target(move |_| system::reveal(std::slice::from_ref(&path))));
                    add(&row, &rv);
                } else {
                    let pad = NSView::new(mtm);
                    size(&pad, 44.0, 1.0);
                    add(&row, &pad);
                }
                add_full(&legend, &row);
            }
            if pie {
                let row = hstack(mtm, 36.0);
                row.setAlignment(NSLayoutAttribute::CenterY);
                let chart = PieChart::new(mtm, 300.0, parts.iter().zip(&colors).map(|(p, c)| (p.bytes as f64, c.clone())).collect());
                chart.setAccessibilityLabel(Some(&ns(&title)));
                add(&row, &chart);
                add(&row, &legend);
                legend.widthAnchor().constraintGreaterThanOrEqualToConstant(360.0).setActive(true);
                add_full(&inner, &row);
            } else {
                let bars = parts
                    .iter()
                    .filter(|p| p.label != "Free")
                    .map(|p| (part_label(&p.label), p.bytes as f64, format::bytes(p.bytes)))
                    .collect();
                let chart = ColumnChart::new(mtm, 280.0, bars, &series_color(0));
                chart.setAccessibilityLabel(Some(&ns(&title)));
                add_full(&inner, &chart);
                add_full(&inner, &separator(mtm));
                add_full(&inner, &legend);
            }
            let when = chrono::DateTime::parse_from_rfc3339(&u.at)
                .map(|t| t.with_timezone(&chrono::Local).format("%d/%m/%Y %H:%M").to_string())
                .unwrap_or_default();
            let icloud = a.settings.borrow().include_icloud;
            let note = if scope == 0 && !icloud {
                tr(
                    &format!("Đo lúc {when}. Bỏ qua iCloud Drive, thư mục đám mây và ~/Library/Containers để macOS không hỏi quyền. Bật \"Đo cả iCloud Drive\" trong Cài đặt để tính cả iCloud."),
                    &format!("Measured {when}. iCloud Drive, cloud folders and ~/Library/Containers are skipped so macOS doesn't ask for access. Turn on \"Include iCloud Drive in Storage\" in Settings to count iCloud."),
                )
            } else if scope == 0 {
                tr(
                    &format!("Đo lúc {when}. Có tính iCloud Drive; bỏ qua ~/Library/Containers và Group Containers."),
                    &format!("Measured {when}. iCloud Drive included; ~/Library/Containers and Group Containers are skipped."),
                )
            } else {
                tr(
                    &format!("Đo lúc {when}. \"Hệ thống và khác\" là phần đã dùng ngoài Home, Ứng dụng và Thư viện: macOS, bản chụp, người dùng khác."),
                    &format!("Measured {when}. \"System & other\" is used space outside Home, Applications and Library: macOS, snapshots, other users."),
                )
            };
            add(&inner, &wrapping(mtm, &note, &font(12.0, W::Regular), &Palette::tertiary()));
        }
    }
    add_full(&s, &c);
    a.refs.borrow_mut().usage_status = Some(status);
    let sc = scroller(mtm, &s);
    frame(a, &tr("Dung lượng thư mục", "Storage"), &tr("Thư mục nào đang chiếm chỗ", "Which folders take up space"), acc, &sc)
}

// ================================================================ daily changes

/// "+1.2 GB" / "−300 MB" / "0 B".
fn signed_bytes(d: i64) -> String {
    match d.signum() {
        1 => format!("+{}", format::bytes(d)),
        -1 => format!("−{}", format::bytes(-d)),
        _ => "0 B".into(),
    }
}

fn delta_color(d: i64) -> Retained<NSColor> {
    match d.signum() {
        1 => Palette::red(),
        -1 => Palette::green(),
        _ => Palette::secondary(),
    }
}

fn day_text(day: &str) -> String {
    chrono::NaiveDate::parse_from_str(day, "%Y-%m-%d")
        .map(|d| {
            let wd = d.format("%a").to_string();
            format!("{} · {}", d.format("%d/%m/%Y"), wd)
        })
        .unwrap_or_else(|_| day.to_string())
}

fn fixed(v: &NSView, w: f64) -> Retained<NSView> {
    size(v, w, 0.0);
    v.retain()
}

fn changes_page(a: &App) -> Retained<NSView> {
    let mtm = a.mtm;
    let snaps: Vec<Snapshot> = a.store.snapshots(31).unwrap_or_default();
    let s = page_stack(mtm);
    let subtitle = tr(
        "Mỗi ngày lưu một bản ghi sau lần quét cuối · dung lượng đã dùng tính cho cả ổ, chi tiết thư mục chỉ trong thư mục quét",
        "One snapshot per day from the last scan · used space covers the whole volume, folder details cover scan folders only",
    );
    if snaps.is_empty() {
        let (c, inner) = card(mtm, 18.0, 6.0);
        add(&inner, &label(mtm, &tr("Chưa có bản ghi nào.", "No snapshots yet."), &font(14.0, W::Semibold), &Palette::primary()));
        add(&inner, &wrapping(mtm, &tr("Bản ghi đầu tiên được lưu sau lần quét tới. Thay đổi hiện ra từ ngày thứ hai.", "The first snapshot is saved after the next scan. Changes appear from the second day."), &font(13.0, W::Regular), &Palette::secondary()));
        add_full(&s, &c);
        let sc = scroller(mtm, &s);
        return frame(a, &tr("Thay đổi theo ngày", "Daily changes"), &subtitle, vec![scan_button(a)], &sc);
    }
    let pick = a.changes_day.get().min(snaps.len() - 1);

    // Day by day
    let (c1, inner) = card(mtm, 18.0, 0.0);
    add_full(&inner, &card_header(mtm, "calendar", &Palette::cyan(), &tr("Từng ngày", "Day by day"), None));
    inner.setCustomSpacing_afterView(10.0, &inner.arrangedSubviews().lastObject().unwrap());
    let head = hstack(mtm, 12.0);
    head.setEdgeInsets(NSEdgeInsets { top: 0.0, left: 10.0, bottom: 0.0, right: 10.0 });
    head.heightAnchor().constraintEqualToConstant(28.0).setActive(true);
    let hdr = |t: &str| label(mtm, t, &font(12.0, W::Semibold), &Palette::secondary());
    add(&head, &fixed(&hdr(&tr("Ngày", "Day")), 170.0));
    add(&head, &fixed(&hdr(&tr("Đã dùng", "Used")), 110.0));
    add(&head, &fixed(&hdr(&tr("Thay đổi", "Change")), 110.0));
    add(&head, &fixed(&hdr(&tr("Còn trống", "Free")), 110.0));
    add(&head, &hdr(&tr("Tệp lớn", "Large files")));
    add(&head, &spacer(mtm));
    add_full(&inner, &head);
    add_full(&inner, &separator(mtm));
    for (i, snap) in snaps.iter().enumerate() {
        let row = ClickView::new(mtm, &Palette::tint(&NSColor::controlAccentColor(), 0.14), 6.0);
        row.heightAnchor().constraintEqualToConstant(36.0).setActive(true);
        row.on_click(move || {
            let a = app();
            a.changes_day.set(i);
            a.request_render();
        });
        row.set_selected(i == pick);
        let h = hstack(mtm, 12.0);
        h.setEdgeInsets(NSEdgeInsets { top: 0.0, left: 10.0, bottom: 0.0, right: 10.0 });
        add(&h, &fixed(&label(mtm, &day_text(&snap.day), &digits(13.0, W::Regular), &Palette::primary()), 170.0));
        add(&h, &fixed(&label(mtm, &format::bytes(snap.used()), &digits(13.0, W::Semibold), &Palette::primary()), 110.0));
        let (dt, dc) = match snaps.get(i + 1) {
            Some(prev) => {
                let d = snap.used() - prev.used();
                (signed_bytes(d), delta_color(d))
            }
            None => ("—".to_string(), Palette::tertiary()),
        };
        add(&h, &fixed(&label(mtm, &dt, &digits(13.0, W::Semibold), &dc), 110.0));
        let free: i64 = snap.volumes.iter().map(|v| v.free_bytes).sum();
        add(&h, &fixed(&label(mtm, &format::bytes(free), &digits(13.0, W::Regular), &Palette::secondary()), 110.0));
        let total: i64 = snap.files.iter().map(|f| f.size).sum();
        add(&h, &label(mtm, &format!("{} · {}", snap.files.len(), format::bytes(total)), &digits(13.0, W::Regular), &Palette::secondary()));
        add(&h, &spacer(mtm));
        row.addSubview(&h);
        pin(&h, &row, 0.0);
        add_full(&inner, &row);
    }
    add_full(&s, &c1);

    // Details for the picked day
    let snap = &snaps[pick];
    let (c2, inner) = card(mtm, 18.0, 4.0);
    let home = a.home.to_string_lossy().to_string();
    let short = |p: &str| p.replace(&home, "~");
    let line = |left: &str, mid: String, right: String, color: Retained<NSColor>| {
        let h = hstack(mtm, 12.0);
        h.heightAnchor().constraintEqualToConstant(30.0).setActive(true);
        let l = label(mtm, left, &font(13.0, W::Regular), &Palette::primary());
        l.setLineBreakMode(NSLineBreakMode::ByTruncatingMiddle);
        l.setToolTip(Some(&ns(left)));
        l.setContentCompressionResistancePriority_forOrientation(100.0, NSLayoutConstraintOrientation::Horizontal);
        add(&h, &l);
        add(&h, &spacer(mtm));
        add(&h, &label(mtm, &mid, &digits(12.0, W::Regular), &Palette::secondary()));
        let r = label(mtm, &right, &digits(13.0, W::Semibold), &color);
        r.setAlignment(NSTextAlignment::Right);
        add(&h, &fixed(&r, 96.0));
        h
    };
    let sub = |t: &str| label(mtm, t, &font(13.0, W::Semibold), &Palette::secondary());
    match snaps.get(pick + 1) {
        None => {
            add_full(&inner, &card_header(mtm, "folder.fill", &Palette::blue(), &tr(&format!("Kích thước thư mục · {}", day_text(&snap.day)), &format!("Folder sizes · {}", day_text(&snap.day))), None));
            add(&inner, &label(mtm, &tr("Chưa có ngày trước để so sánh; đây là kích thước hiện tại.", "No earlier day to compare yet; these are the current sizes."), &font(12.0, W::Regular), &Palette::tertiary()));
            for (p, size) in snap.folders.iter().take(15) {
                add_full(&inner, &line(&short(p), String::new(), format::bytes(*size), Palette::primary()));
            }
        }
        Some(prev) => {
            let ch = compare(prev, snap);
            add_full(&inner, &card_header(
                mtm,
                "arrow.left.arrow.right",
                &Palette::blue(),
                &tr(&format!("{} so với {}", day_text(&snap.day), day_text(&prev.day)), &format!("{} vs {}", day_text(&snap.day), day_text(&prev.day))),
                Some(label(mtm, &signed_bytes(ch.used_delta), &digits(17.0, W::Semibold), &delta_color(ch.used_delta)).into_view()),
            ));
            for (v, pv) in snap.volumes.iter().map(|v| (v, prev.volumes.iter().find(|p| p.path == v.path))) {
                if let Some(pv) = pv {
                    let d = v.used() - pv.used();
                    add_full(&inner, &line(&v.name, format!("{} → {}", format::bytes(pv.used()), format::bytes(v.used())), signed_bytes(d), delta_color(d)));
                }
            }
            add_full(&inner, &separator(mtm));
            add(&inner, &sub(&tr("Thư mục thay đổi nhiều nhất", "Folders that changed most")));
            if ch.folders.is_empty() {
                add(&inner, &label(mtm, &tr("Không có thay đổi trong thư mục quét.", "No changes in scan folders."), &font(12.0, W::Regular), &Palette::tertiary()));
            }
            for (p, before, after) in ch.folders.iter().take(12) {
                let d = after - before;
                add_full(&inner, &line(&short(p), format!("{} → {}", format::bytes(*before), format::bytes(*after)), signed_bytes(d), delta_color(d)));
            }
            add_full(&inner, &separator(mtm));
            add(&inner, &sub(&tr(&format!("Tệp lớn mới ({})", ch.new_files.len()), &format!("New large files ({})", ch.new_files.len()))));
            for f in ch.new_files.iter().take(10) {
                add_full(&inner, &line(&short(&f.path), String::new(), format!("+{}", format::bytes(f.size)), Palette::red()));
            }
            add(&inner, &sub(&tr(&format!("Tệp lớn đã mất ({})", ch.gone_files.len()), &format!("Large files gone ({})", ch.gone_files.len()))));
            for f in ch.gone_files.iter().take(10) {
                add_full(&inner, &line(&short(&f.path), String::new(), format!("−{}", format::bytes(f.size)), Palette::green()));
            }
        }
    }
    add_full(&s, &c2);
    let sc = scroller(mtm, &s);
    frame(a, &tr("Thay đổi theo ngày", "Daily changes"), &subtitle, vec![scan_button(a)], &sc)
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
    let toggles: [Toggle; 5] = [
        (tr("Tự động quét", "Auto scan"), Some(tr("Chỉ quét và thông báo, không tự xoá", "Scans and notifies only, never deletes")), d.auto_scan, |s, v| s.auto_scan = v),
        (tr("Mở khi đăng nhập", "Open at login"), None, d.open_at_login, |s, v| s.open_at_login = v),
        (tr("Đánh giá bằng Claude CLI", "Review with Claude CLI"), Some(tr("Chỉ gửi đường dẫn, kích thước, ngày", "Sends only paths, sizes and dates")), d.ai_review, |s, v| s.ai_review = v),
        (tr("Quét toàn bộ Home", "Scan all of Home"), Some(tr("Trừ các vùng được bảo vệ", "Except protected areas")), d.scan_home, |s, v| s.scan_home = v),
        (tr("Đo cả iCloud Drive", "Include iCloud Drive in Storage"), Some(tr("macOS sẽ hỏi quyền truy cập · không bao giờ dọn", "macOS will ask for access · never cleaned")), d.include_icloud, |s, v| s.include_icloud = v),
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
        if i == 0 {
            // Interval right under the auto-scan toggle.
            add_full(&inner, &separator(mtm));
            let h = hstack(mtm, 8.0);
            h.heightAnchor().constraintEqualToConstant(50.0).setActive(true);
            add(&h, &label(mtm, &tr("Quét mỗi", "Scan every"), &font(14.0, W::Regular), &Palette::primary()));
            add(&h, &spacer(mtm));
            const CHOICES: [i64; 5] = [5, 10, 30, 60, 120];
            let labels: Vec<String> = CHOICES.iter().map(|m| interval_text(*m)).collect();
            let refs: Vec<&str> = labels.iter().map(String::as_str).collect();
            let sel = CHOICES.iter().position(|m| *m == d.scan_interval_minutes).map(|i| i as isize).unwrap_or(-1);
            let seg = segmented(mtm, &refs, sel, &a.target(|_| {}));
            let seg_ref = seg.clone();
            let t = a.target(move |a| {
                let i = seg_ref.selectedSegment();
                if let Some(m) = usize::try_from(i).ok().and_then(|i| CHOICES.get(i)) {
                    a.edit_draft(|s| s.scan_interval_minutes = *m);
                }
            });
            unsafe { seg.setTarget(Some(&t)) };
            add(&h, &seg);
            add_full(&inner, &h);
        }
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
    add_full(&inner, &separator(mtm));
    add(&inner, &label(mtm, &tr("Thư mục riêng tư được phép đọc", "Private folders the app may read"), &font(13.0, W::Semibold), &Palette::primary()));
    let pf = hstack(mtm, 16.0);
    for f in PRIVATE_FOLDERS {
        let name = match f {
            "Desktop" => tr("Màn hình nền", "Desktop"),
            "Documents" => tr("Tài liệu", "Documents"),
            _ => tr("Tải về", "Downloads"),
        };
        let cb = unsafe { NSButton::checkboxWithTitle_target_action(&ns(&name), None, None, mtm) };
        cb.setState(if d.private_folders.iter().any(|p| p == f) { NSControlStateValueOn } else { NSControlStateValueOff });
        let cb_ref = cb.clone();
        let t = a.target(move |a| {
            let on = cb_ref.state() == NSControlStateValueOn;
            a.edit_draft(|s| {
                s.private_folders.retain(|p| p != f);
                if on {
                    s.private_folders.push(f.to_string());
                }
            });
        });
        unsafe {
            cb.setTarget(Some(&t));
            cb.setAction(Some(fire_sel()));
        }
        add(&pf, &cb);
    }
    add(&inner, &pf);
    add(&inner, &wrapping(mtm, &tr("macOS chỉ hỏi quyền cho thư mục được chọn. Thư mục không chọn sẽ không bao giờ được mở, kể cả khi quét toàn bộ Home.", "macOS only asks about the folders ticked here. Unticked folders are never opened, even when scanning all of Home."), &font(12.0, W::Regular), &Palette::tertiary()));
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
    add_full(&s, &menu_bar_settings(a, &d));
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

/// Settings card: what the menu bar item shows next to its dot.
fn menu_bar_settings(a: &App, d: &Settings) -> Retained<NSBox> {
    let mtm = a.mtm;
    let (c, inner) = card(mtm, 18.0, 4.0);
    add(&inner, &label(mtm, &tr("Thanh menu", "Menu bar"), &font(15.0, W::Semibold), &Palette::primary()));
    add(&inner, &label(mtm, &tr("Chọn giá trị hiện cạnh chấm trạng thái", "Choose what shows next to the status dot"), &font(12.0, W::Regular), &Palette::secondary()));
    let row = hstack(mtm, 18.0);
    row.heightAnchor().constraintEqualToConstant(44.0).setActive(true);
    for id in sdc_core::overview::MENU_BAR_ITEMS {
        let name = match id {
            "disk" => tr("Ổ trống", "Disk free"),
            "temp" => tr("Tệp tạm", "Temporary"),
            "cpu" => "CPU".to_string(),
            _ => tr("Bộ nhớ", "Memory"),
        };
        let on = d.menu_bar_items.iter().any(|x| x == id);
        let cb = unsafe { NSButton::checkboxWithTitle_target_action(&ns(&format!("{name} ({})", a.metric_text(id))), None, None, mtm) };
        cb.setState(if on { NSControlStateValueOn } else { NSControlStateValueOff });
        let cb_ref = cb.clone();
        let t = a.target(move |a| {
            let v = cb_ref.state() == NSControlStateValueOn;
            a.edit_draft(|s| {
                s.menu_bar_items.retain(|x| x != id);
                if v {
                    s.menu_bar_items.push(id.to_string());
                }
            });
        });
        unsafe {
            cb.setTarget(Some(&t));
            cb.setAction(Some(fire_sel()));
        }
        add(&row, &cb);
    }
    add_full(&inner, &row);
    add_full(&inner, &separator(mtm));
    let h = hstack(mtm, 8.0);
    h.heightAnchor().constraintEqualToConstant(48.0).setActive(true);
    add(&h, &label(mtm, &tr("Kiểu hiển thị", "Style"), &font(14.0, W::Regular), &Palette::primary()));
    add(&h, &spacer(mtm));
    let labels = [tr("Giá trị", "Values"), tr("Có nhãn", "Labels"), tr("Biểu tượng", "Icons")];
    let refs: Vec<&str> = labels.iter().map(String::as_str).collect();
    let sel = sdc_core::overview::MENU_BAR_STYLES.iter().position(|x| *x == d.menu_bar_style).unwrap_or(0) as isize;
    let seg = segmented(mtm, &refs, sel, &a.target(|_| {}));
    let seg_ref = seg.clone();
    let t = a.target(move |a| {
        if let Some(st) = usize::try_from(seg_ref.selectedSegment()).ok().and_then(|i| sdc_core::overview::MENU_BAR_STYLES.get(i)) {
            a.edit_draft(|s| s.menu_bar_style = st.to_string());
        }
    });
    unsafe { seg.setTarget(Some(&t)) };
    add(&h, &seg);
    add_full(&inner, &h);
    add_full(&inner, &separator(mtm));
    let h = hstack(mtm, 8.0);
    h.heightAnchor().constraintEqualToConstant(48.0).setActive(true);
    let t = vstack(mtm, 2.0);
    add(&t, &label(mtm, &tr("Chấm trạng thái", "Status dot"), &font(14.0, W::Regular), &Palette::primary()));
    add(&t, &label(mtm, &tr("Xanh, vàng, đỏ theo dung lượng trống", "Green, yellow, red by free space"), &font(12.0, W::Regular), &Palette::secondary()));
    add(&h, &t);
    add(&h, &spacer(mtm));
    let sw = switch(mtm, d.menu_bar_dot, &a.target(|_| {}));
    let sw_ref = sw.clone();
    let tg = a.target(move |a| {
        let v = sw_ref.state() == NSControlStateValueOn;
        a.edit_draft(|s| s.menu_bar_dot = v);
    });
    unsafe { sw.setTarget(Some(&tg)) };
    add(&h, &sw);
    add_full(&inner, &h);
    c
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
    add(&s, &label(mtm, &if status.is_empty() { tr("Bật tự quét mỗi 10 phút và mở khi đăng nhập · đổi được trong Cài đặt", "Auto-scan every 10 minutes and open at login · change in Settings") } else { status }, &font(12.0, W::Regular), &Palette::tertiary()));
    let _ = ProtocolObject::<dyn NSObjectProtocol>::from_ref(&*outer);
    outer
}
