//! macOS integrations: Trash, Finder, login item, notifications, tray icon.

use crate::ui::ns;
use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::{Bool, ProtocolObject};
use objc2::{AnyThread, MainThreadMarker};
use objc2_app_kit::*;
use objc2_foundation::*;
use objc2_service_management::{SMAppService, SMAppServiceStatus};
use objc2_user_notifications::*;
use sdc_core::model::Level;

pub fn move_to_trash(path: &str) -> Result<(), String> {
    let url = NSURL::fileURLWithPath(&ns(path));
    NSFileManager::defaultManager()
        .trashItemAtURL_resultingItemURL_error(&url, None)
        .map_err(|e| e.localizedDescription().to_string())
}

pub fn reveal(paths: &[String]) {
    let urls: Vec<Retained<NSURL>> = paths.iter().map(|p| NSURL::fileURLWithPath(&ns(p))).collect();
    NSWorkspace::sharedWorkspace().activateFileViewerSelectingURLs(&NSArray::from_retained_slice(&urls));
}

pub fn app_icon(path: &str) -> Retained<NSImage> {
    NSWorkspace::sharedWorkspace().iconForFile(&ns(path))
}

fn bundled() -> bool {
    NSBundle::mainBundle().bundleIdentifier().is_some()
}

/// Register/unregister as a login item (macOS 13+, needs a bundled app).
pub fn set_open_at_login(on: bool) -> Result<(), String> {
    if !bundled() {
        return Ok(());
    }
    let svc = unsafe { SMAppService::mainAppService() };
    let enabled = unsafe { svc.status() } == SMAppServiceStatus::Enabled;
    let r = unsafe {
        match (on, enabled) {
            (true, false) => svc.registerAndReturnError(),
            (false, true) => svc.unregisterAndReturnError(),
            _ => Ok(()),
        }
    };
    r.map_err(|e| e.localizedDescription().to_string())
}

/// True when this launch came from the login-items list.
pub fn launched_as_login_item() -> bool {
    const KEY_AE_PROP_DATA: u32 = u32::from_be_bytes(*b"prdt");
    const LOGIN_ITEM: u32 = u32::from_be_bytes(*b"lgit");
    let mgr = NSAppleEventManager::sharedAppleEventManager();
    let Some(event) = (unsafe { mgr.currentAppleEvent() }) else { return false };
    if unsafe { event.eventID() } != u32::from_be_bytes(*b"oapp") {
        return false;
    }
    unsafe { event.paramDescriptorForKeyword(KEY_AE_PROP_DATA) }
        .map(|d| unsafe { d.enumCodeValue() } == LOGIN_ITEM)
        .unwrap_or(false)
}

pub fn notifications_setup(delegate: &ProtocolObject<dyn UNUserNotificationCenterDelegate>) {
    if !bundled() {
        return;
    }
    let center = UNUserNotificationCenter::currentNotificationCenter();
    center.setDelegate(Some(delegate));
    let block = RcBlock::new(|_granted: Bool, _err: *mut NSError| {});
    center.requestAuthorizationWithOptions_completionHandler(
        UNAuthorizationOptions::Alert | UNAuthorizationOptions::Sound,
        &block,
    );
}

pub fn notify(title: &str, body: &str) {
    if !bundled() {
        return;
    }
    let content = UNMutableNotificationContent::new();
    content.setTitle(&ns(title));
    content.setBody(&ns(body));
    let req = UNNotificationRequest::requestWithIdentifier_content_trigger(&ns("cleanup-suggestions"), &content, None);
    UNUserNotificationCenter::currentNotificationCenter().addNotificationRequest_withCompletionHandler(&req, None);
}

pub fn clear_notifications() {
    if bundled() {
        UNUserNotificationCenter::currentNotificationCenter().removeAllDeliveredNotifications();
    }
}

/// Colored status dot for the menu bar.
pub fn tray_image(level: Level) -> Retained<NSImage> {
    let color = match level {
        Level::Green => NSColor::systemGreenColor(),
        Level::Yellow => NSColor::systemYellowColor(),
        Level::Red => NSColor::systemRedColor(),
    };
    let block = RcBlock::new(move |rect: NSRect| -> Bool {
        let inset = NSRect::new(
            NSPoint::new(rect.origin.x + 3.5, rect.origin.y + 3.5),
            NSSize::new(rect.size.width - 7.0, rect.size.height - 7.0),
        );
        color.setFill();
        NSBezierPath::bezierPathWithOvalInRect(inset).fill();
        NSColor::colorWithWhite_alpha(0.0, 0.25).setStroke();
        let ring = NSBezierPath::bezierPathWithOvalInRect(inset);
        ring.setLineWidth(1.0);
        ring.stroke();
        Bool::YES
    });
    let img = NSImage::imageWithSize_flipped_drawingHandler(NSSize::new(18.0, 18.0), false, &block);
    img.setTemplate(false);
    img
}

pub fn preferred_vietnamese() -> bool {
    NSLocale::preferredLanguages().firstObject().map(|l| l.to_string().starts_with("vi")).unwrap_or(false)
}

pub fn _mtm() -> MainThreadMarker {
    MainThreadMarker::new().expect("main thread")
}

/// Menu bar title with an SF Symbol before each value; symbols follow the text color.
pub fn icon_title(parts: &[(&str, String)], font: &NSFont) -> Retained<NSAttributedString> {
    let out = NSMutableAttributedString::new();
    let attrs = unsafe {
        NSDictionary::<NSAttributedStringKey, objc2::runtime::AnyObject>::from_slices(
            &[NSFontAttributeName, NSForegroundColorAttributeName],
            &[font as &objc2::runtime::AnyObject, &*NSColor::labelColor() as &objc2::runtime::AnyObject],
        )
    };
    for (i, (symbol, value)) in parts.iter().enumerate() {
        if let Some(img) = crate::ui::symbol(symbol, 12.0, crate::ui::W::Regular) {
            img.setTemplate(true);
            let att = NSTextAttachment::new();
            att.setImage(Some(&img));
            let s = NSAttributedString::attributedStringWithAttachment(&att);
            let m = NSMutableAttributedString::initWithAttributedString(NSMutableAttributedString::alloc(), &s);
            unsafe { m.addAttributes_range(&attrs, NSRange::new(0, m.length())) };
            out.appendAttributedString(&m);
        }
        let text = format!(" {value}{}", if i + 1 < parts.len() { "   " } else { "" });
        let piece = unsafe { NSAttributedString::initWithString_attributes(NSAttributedString::alloc(), &ns(&text), Some(&attrs)) };
        out.appendAttributedString(&piece);
    }
    out.into_super()
}
