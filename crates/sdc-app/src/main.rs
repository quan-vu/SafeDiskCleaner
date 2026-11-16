//! Safety Disk Cleaner — native macOS app (Rust + AppKit via objc2).
#![allow(unused_unsafe)]

mod app;
mod pages;
mod scan_sheet;
mod system;
mod ui;

use app::{app, tr, App, Page};
use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, ProtocolObject};
use objc2::{define_class, msg_send, sel, MainThreadMarker, MainThreadOnly};
use objc2_app_kit::*;
use objc2_foundation::*;
use objc2_user_notifications::*;
use std::ptr::NonNull;
use ui::ns;

const BUNDLE_ID: &str = "io.github.quan-vu.safetydiskcleaner";
const SHOW_NOTE: &str = "io.github.quan-vu.safetydiskcleaner.show";

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "SDCAppDelegate"]
    struct Delegate;

    unsafe impl NSObjectProtocol for Delegate {}

    unsafe impl NSApplicationDelegate for Delegate {
        #[unsafe(method(applicationDidFinishLaunching:))]
        fn did_finish_launching(&self, _n: &NSNotification) {
            launch(self);
        }

        #[unsafe(method(applicationShouldHandleReopen:hasVisibleWindows:))]
        fn reopen(&self, _app: &NSApplication, _visible: bool) -> bool {
            app().show_window();
            true
        }

        #[unsafe(method(applicationShouldTerminateAfterLastWindowClosed:))]
        fn keep_running(&self, _app: &NSApplication) -> bool {
            false
        }

        #[unsafe(method(applicationWillTerminate:))]
        fn will_terminate(&self, _n: &NSNotification) {
            app().cancel.borrow().store(true, std::sync::atomic::Ordering::Relaxed);
        }
    }

    unsafe impl NSWindowDelegate for Delegate {
        /// Closing only hides the window; the app keeps running in the menu bar.
        #[unsafe(method(windowShouldClose:))]
        fn should_close(&self, sender: &NSWindow) -> bool {
            sender.orderOut(None);
            false
        }
    }

    unsafe impl UNUserNotificationCenterDelegate for Delegate {
        #[unsafe(method(userNotificationCenter:didReceiveNotificationResponse:withCompletionHandler:))]
        fn did_receive(
            &self,
            _center: &UNUserNotificationCenter,
            _response: &UNNotificationResponse,
            handler: &block2::DynBlock<dyn Fn()>,
        ) {
            let a = app();
            a.show_window();
            a.navigate(Page::Files);
            handler.call(());
        }

        #[unsafe(method(userNotificationCenter:willPresentNotification:withCompletionHandler:))]
        fn will_present(
            &self,
            _center: &UNUserNotificationCenter,
            _n: &UNNotification,
            handler: &block2::DynBlock<dyn Fn(UNNotificationPresentationOptions)>,
        ) {
            handler.call((UNNotificationPresentationOptions::Banner | UNNotificationPresentationOptions::List,));
        }
    }
);

fn launch(delegate: &Delegate) {
    let mtm = MainThreadMarker::new().expect("main thread");
    let a = match App::init(mtm) {
        Ok(a) => a,
        Err(e) => {
            let alert = NSAlert::new(mtm);
            alert.setMessageText(&ns("Safety Disk Cleaner"));
            alert.setInformativeText(&ns(&e));
            alert.runModal();
            NSApplication::sharedApplication(mtm).terminate(None);
            return;
        }
    };
    build_menu(mtm);
    let window = pages::build_window(&a);
    window.setDelegate(Some(ProtocolObject::from_ref(delegate)));
    *a.window.borrow_mut() = Some(window.clone());
    pages::build_tray(&a);
    system::notifications_setup(ProtocolObject::from_ref(delegate));

    // A second launch asks this instance to show its window.
    let block = RcBlock::new(|_n: NonNull<NSNotification>| app().show_window());
    unsafe {
        NSDistributedNotificationCenter::defaultCenter().addObserverForName_object_queue_usingBlock(
            Some(&ns(SHOW_NOTE)),
            None,
            Some(&NSOperationQueue::mainQueue()),
            &block,
        );
    }

    a.start_sampler();
    a.update_footer();
    let setup_done = a.settings.borrow().setup_done;
    if setup_done {
        a.refresh_volumes();
    }
    a.update_sidebar_counts();
    a.navigate(Page::Overview);
    if !(setup_done && system::launched_as_login_item()) {
        a.show_window();
    }

    // 1 Hz live refresh; auto-scan check every minute.
    let ticks = std::cell::Cell::new(0u64);
    let tick = RcBlock::new(move |_t: NonNull<NSTimer>| {
        let a = app();
        a.tick();
        let n = ticks.get() + 1;
        ticks.set(n);
        if n.is_multiple_of(60) {
            a.refresh_volumes();
            a.update_sidebar_counts();
            a.maybe_scan();
        }
    });
    let timer = unsafe { NSTimer::scheduledTimerWithTimeInterval_repeats_block(1.0, true, &tick) };
    unsafe { NSRunLoop::mainRunLoop().addTimer_forMode(&timer, NSRunLoopCommonModes) };
    std::mem::forget(timer);
    a.maybe_scan();
}

fn build_menu(mtm: MainThreadMarker) {
    let bar = NSMenu::new(mtm);
    let item = |title: &str, action: objc2::runtime::Sel, key: &str| unsafe {
        NSMenuItem::initWithTitle_action_keyEquivalent(NSMenuItem::alloc(mtm), &ns(title), Some(action), &ns(key))
    };
    let submenu = |title: &str, items: Vec<Retained<NSMenuItem>>| {
        let m = NSMenu::initWithTitle(NSMenu::alloc(mtm), &ns(title));
        for i in items {
            m.addItem(&i);
        }
        let top = NSMenuItem::new(mtm);
        top.setSubmenu(Some(&m));
        bar.addItem(&top);
    };
    submenu(
        "Safety Disk Cleaner",
        vec![
            item(&tr("Ẩn cửa sổ", "Close Window"), sel!(performClose:), "w"),
            item(&tr("Thoát Safety Disk Cleaner", "Quit Safety Disk Cleaner"), sel!(terminate:), "q"),
        ],
    );
    submenu(
        "Edit",
        vec![
            item("Undo", sel!(undo:), "z"),
            item("Cut", sel!(cut:), "x"),
            item("Copy", sel!(copy:), "c"),
            item("Paste", sel!(paste:), "v"),
            item("Select All", sel!(selectAll:), "a"),
        ],
    );
    NSApplication::sharedApplication(mtm).setMainMenu(Some(&bar));
}

/// Another instance already running? Ask it to show itself and report true.
fn hand_off_to_running_instance() -> bool {
    let me = std::process::id() as i32;
    let others = NSRunningApplication::runningApplicationsWithBundleIdentifier(&ns(BUNDLE_ID));
    let running = others.iter().any(|r| r.processIdentifier() != me);
    if running {
        unsafe {
            NSDistributedNotificationCenter::defaultCenter().postNotificationName_object_userInfo_deliverImmediately(
                &ns(SHOW_NOTE),
                None,
                None,
                true,
            );
        }
    }
    running
}

fn main() {
    let mtm = MainThreadMarker::new().expect("must run on the main thread");
    if hand_off_to_running_instance() {
        return;
    }
    let app = NSApplication::sharedApplication(mtm);
    app.setActivationPolicy(NSApplicationActivationPolicy::Regular);
    let delegate: Retained<Delegate> = unsafe { msg_send![Delegate::alloc(mtm), init] };
    app.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
    let _: Option<&AnyObject> = None;
    app.run();
}
