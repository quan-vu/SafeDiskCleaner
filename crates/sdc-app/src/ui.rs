//! AppKit building blocks: closure targets, custom views and small helpers that
//! keep page code declarative.

use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, NSObject};
use objc2::{define_class, msg_send, sel, AllocAnyThread, ClassType, DefinedClass, MainThreadMarker, MainThreadOnly, Message};
use objc2_app_kit::*;
use objc2_foundation::*;
use sdc_core::sysstats::Sample;
use std::cell::{Cell, RefCell};
use std::ptr::NonNull;

// ---------------------------------------------------------------- targets

type Callback = Box<dyn Fn(Option<&AnyObject>)>;

pub struct TargetIvars {
    f: Callback,
}

define_class!(
    /// Objective-C target that forwards `fire:` to a Rust closure.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "SDCTarget"]
    #[ivars = TargetIvars]
    pub struct Target;

    impl Target {
        #[unsafe(method(fire:))]
        fn fire(&self, sender: Option<&AnyObject>) {
            (self.ivars().f)(sender);
        }
    }

    unsafe impl NSObjectProtocol for Target {}

    unsafe impl NSTextFieldDelegate for Target {}
    unsafe impl NSControlTextEditingDelegate for Target {
        #[unsafe(method(controlTextDidChange:))]
        fn text_did_change(&self, notification: &NSNotification) {
            let obj = notification.object();
            (self.ivars().f)(obj.as_deref());
        }
    }
);

impl Target {
    pub fn new(mtm: MainThreadMarker, f: impl Fn(Option<&AnyObject>) + 'static) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(TargetIvars { f: Box::new(f) });
        unsafe { msg_send![super(this), init] }
    }
}

pub fn fire_sel() -> objc2::runtime::Sel {
    sel!(fire:)
}

/// Upcast any view type to `Retained<NSView>`.
pub trait IntoView {
    fn into_view(self) -> Retained<NSView>;
}
impl<T: AsRef<NSView> + Message> IntoView for Retained<T> {
    fn into_view(self) -> Retained<NSView> {
        (*self).as_ref().retain()
    }
}

// ---------------------------------------------------------------- colors

fn srgb(hex: u32, alpha: f64) -> Retained<NSColor> {
    let r = ((hex >> 16) & 0xff) as f64 / 255.0;
    let g = ((hex >> 8) & 0xff) as f64 / 255.0;
    let b = (hex & 0xff) as f64 / 255.0;
    NSColor::colorWithSRGBRed_green_blue_alpha(r, g, b, alpha)
}

/// Color that follows the effective appearance (light / dark).
pub fn dyn_color(light: u32, dark: u32) -> Retained<NSColor> {
    dyn_color_a(light, 1.0, dark, 1.0)
}

pub fn dyn_color_a(light: u32, la: f64, dark: u32, da: f64) -> Retained<NSColor> {
    let l = srgb(light, la);
    let d = srgb(dark, da);
    let block = RcBlock::new(move |appearance: NonNull<NSAppearance>| -> NonNull<NSColor> {
        let name = unsafe { appearance.as_ref() }.name().to_string();
        let c = if name.contains("Dark") { &d } else { &l };
        NonNull::from(&**c)
    });
    unsafe { NSColor::colorWithName_dynamicProvider(None, &block) }
}

pub struct Palette;
impl Palette {
    pub fn window() -> Retained<NSColor> {
        dyn_color(0xF2F2F7, 0x1E1E20)
    }
    pub fn card() -> Retained<NSColor> {
        dyn_color(0xFFFFFF, 0x2A2A2D)
    }
    pub fn card_alt() -> Retained<NSColor> {
        dyn_color(0xF7F7FA, 0x2F2F32)
    }
    pub fn track() -> Retained<NSColor> {
        dyn_color(0xE5E5EA, 0x3A3A3D)
    }
    pub fn hairline() -> Retained<NSColor> {
        dyn_color(0xE5E5EA, 0x3A3A3D)
    }
    pub fn primary() -> Retained<NSColor> {
        NSColor::labelColor()
    }
    pub fn secondary() -> Retained<NSColor> {
        NSColor::secondaryLabelColor()
    }
    pub fn tertiary() -> Retained<NSColor> {
        NSColor::tertiaryLabelColor()
    }
    pub fn orange() -> Retained<NSColor> {
        NSColor::systemOrangeColor()
    }
    pub fn cyan() -> Retained<NSColor> {
        NSColor::systemCyanColor()
    }
    pub fn green() -> Retained<NSColor> {
        NSColor::systemGreenColor()
    }
    pub fn yellow() -> Retained<NSColor> {
        dyn_color(0xC79200, 0xFFD60A)
    }
    pub fn red() -> Retained<NSColor> {
        NSColor::systemRedColor()
    }
    pub fn blue() -> Retained<NSColor> {
        NSColor::systemBlueColor()
    }
    pub fn indigo() -> Retained<NSColor> {
        NSColor::systemIndigoColor()
    }
    pub fn purple() -> Retained<NSColor> {
        NSColor::systemPurpleColor()
    }
    pub fn gray() -> Retained<NSColor> {
        NSColor::systemGrayColor()
    }
    pub fn tint(c: &NSColor, alpha: f64) -> Retained<NSColor> {
        c.colorWithAlphaComponent(alpha)
    }
}

// ---------------------------------------------------------------- fonts

#[derive(Clone, Copy)]
pub enum W {
    Regular,
    Medium,
    Semibold,
    Bold,
}

fn weight(w: W) -> NSFontWeight {
    unsafe {
        match w {
            W::Regular => NSFontWeightRegular,
            W::Medium => NSFontWeightMedium,
            W::Semibold => NSFontWeightSemibold,
            W::Bold => NSFontWeightBold,
        }
    }
}

pub fn font(size: f64, w: W) -> Retained<NSFont> {
    NSFont::systemFontOfSize_weight(size, weight(w))
}

pub fn digits(size: f64, w: W) -> Retained<NSFont> {
    NSFont::monospacedDigitSystemFontOfSize_weight(size, weight(w))
}

pub fn mono(size: f64) -> Retained<NSFont> {
    NSFont::monospacedSystemFontOfSize_weight(size, weight(W::Regular))
}

// ---------------------------------------------------------------- text

pub fn ns(s: &str) -> Retained<NSString> {
    NSString::from_str(s)
}

pub fn label(mtm: MainThreadMarker, text: &str, f: &NSFont, color: &NSColor) -> Retained<NSTextField> {
    let l = NSTextField::labelWithString(&ns(text), mtm);
    l.setFont(Some(f));
    l.setTextColor(Some(color));
    l.setLineBreakMode(NSLineBreakMode::ByTruncatingTail);
    l.setTranslatesAutoresizingMaskIntoConstraints(false);
    l.setContentCompressionResistancePriority_forOrientation(250.0, NSLayoutConstraintOrientation::Horizontal);
    l
}

pub fn wrapping(mtm: MainThreadMarker, text: &str, f: &NSFont, color: &NSColor) -> Retained<NSTextField> {
    let l = NSTextField::wrappingLabelWithString(&ns(text), mtm);
    l.setFont(Some(f));
    l.setTextColor(Some(color));
    l.setSelectable(false);
    l.setTranslatesAutoresizingMaskIntoConstraints(false);
    l
}

/// "147" + small "GB" as one attributed label.
pub fn hero(mtm: MainThreadMarker, number: &str, unit: &str, size: f64) -> Retained<NSTextField> {
    let l = label(mtm, "", &digits(size, W::Semibold), &Palette::primary());
    set_hero(&l, number, unit, size);
    l
}

pub fn set_hero(l: &NSTextField, number: &str, unit: &str, size: f64) {
    let s = NSMutableAttributedString::new();
    unsafe {
        let big = NSDictionary::<NSAttributedStringKey, AnyObject>::from_slices(
            &[NSFontAttributeName, NSForegroundColorAttributeName, NSKernAttributeName],
            &[
                &*digits(size, W::Semibold) as &AnyObject,
                &*Palette::primary() as &AnyObject,
                &*NSNumber::new_f64(-0.8) as &AnyObject,
            ],
        );
        let small = NSDictionary::<NSAttributedStringKey, AnyObject>::from_slices(
            &[NSFontAttributeName, NSForegroundColorAttributeName],
            &[&*font((size * 0.42).round(), W::Medium) as &AnyObject, &*Palette::secondary() as &AnyObject],
        );
        s.appendAttributedString(&NSAttributedString::initWithString_attributes(
            NSAttributedString::alloc(),
            &ns(number),
            Some(&big),
        ));
        s.appendAttributedString(&NSAttributedString::initWithString_attributes(
            NSAttributedString::alloc(),
            &ns(&format!(" {unit}")),
            Some(&small),
        ));
    }
    l.setAttributedStringValue(&s);
}

// ---------------------------------------------------------------- images

pub fn symbol(name: &str, size: f64, w: W) -> Option<Retained<NSImage>> {
    let img = NSImage::imageWithSystemSymbolName_accessibilityDescription(&ns(name), None)?;
    let cfg = NSImageSymbolConfiguration::configurationWithPointSize_weight(size, weight(w));
    img.imageWithSymbolConfiguration(&cfg)
}

pub fn symbol_view(mtm: MainThreadMarker, name: &str, size: f64, color: &NSColor) -> Retained<NSImageView> {
    let v = match symbol(name, size, W::Semibold) {
        Some(img) => NSImageView::imageViewWithImage(&img, mtm),
        None => NSImageView::new(mtm),
    };
    v.setContentTintColor(Some(color));
    v.setTranslatesAutoresizingMaskIntoConstraints(false);
    v
}

/// Rounded colored square with a white glyph (sidebar / card header icon).
pub fn icon_tile(mtm: MainThreadMarker, name: &str, bg: &NSColor, fg: &NSColor, side: f64, glyph: f64) -> Retained<NSBox> {
    let b = boxed(mtm, bg, side * 0.28);
    let img = symbol_view(mtm, name, glyph, fg);
    b.addSubview(&img);
    center_in(&img, &b);
    size(&b, side, side);
    b
}

// ---------------------------------------------------------------- layout

pub fn boxed(mtm: MainThreadMarker, fill: &NSColor, radius: f64) -> Retained<NSBox> {
    let b = NSBox::new(mtm);
    b.setBoxType(NSBoxType::Custom);
    b.setTitlePosition(NSTitlePosition::NoTitle);
    b.setFillColor(fill);
    b.setBorderColor(&NSColor::clearColor());
    b.setBorderWidth(0.0);
    b.setCornerRadius(radius);
    b.setContentViewMargins(NSSize::new(0.0, 0.0));
    b.setTranslatesAutoresizingMaskIntoConstraints(false);
    b
}

/// Card: rounded box whose content is a vertical stack with padding.
pub fn card(mtm: MainThreadMarker, padding: f64, spacing: f64) -> (Retained<NSBox>, Retained<NSStackView>) {
    let b = boxed(mtm, &Palette::card(), 14.0);
    let s = vstack(mtm, spacing);
    s.setEdgeInsets(NSEdgeInsets { top: padding, left: padding + 2.0, bottom: padding, right: padding + 2.0 });
    b.addSubview(&s);
    pin(&s, &b, 0.0);
    (b, s)
}

pub fn vstack(mtm: MainThreadMarker, spacing: f64) -> Retained<NSStackView> {
    let s = NSStackView::new(mtm);
    s.setOrientation(NSUserInterfaceLayoutOrientation::Vertical);
    s.setAlignment(NSLayoutAttribute::Leading);
    s.setSpacing(spacing);
    s.setTranslatesAutoresizingMaskIntoConstraints(false);
    s
}

pub fn hstack(mtm: MainThreadMarker, spacing: f64) -> Retained<NSStackView> {
    let s = NSStackView::new(mtm);
    s.setOrientation(NSUserInterfaceLayoutOrientation::Horizontal);
    s.setAlignment(NSLayoutAttribute::CenterY);
    s.setSpacing(spacing);
    s.setTranslatesAutoresizingMaskIntoConstraints(false);
    s
}

/// Horizontal stack whose children share the width equally.
pub fn columns(mtm: MainThreadMarker, spacing: f64) -> Retained<NSStackView> {
    let s = hstack(mtm, spacing);
    s.setDistribution(NSStackViewDistribution::FillEqually);
    s.setAlignment(NSLayoutAttribute::Top);
    s
}

pub fn spacer(mtm: MainThreadMarker) -> Retained<NSView> {
    let v = NSView::new(mtm);
    v.setTranslatesAutoresizingMaskIntoConstraints(false);
    v.setContentHuggingPriority_forOrientation(1.0, NSLayoutConstraintOrientation::Horizontal);
    v.setContentHuggingPriority_forOrientation(1.0, NSLayoutConstraintOrientation::Vertical);
    v.setContentCompressionResistancePriority_forOrientation(1.0, NSLayoutConstraintOrientation::Horizontal);
    v
}

/// Add `child` to a vertical stack and make it span the stack's inner width.
pub fn add_full(stack: &NSStackView, child: &NSView) {
    stack.addArrangedSubview(child);
    let i = stack.edgeInsets();
    child.leadingAnchor().constraintEqualToAnchor_constant(&stack.leadingAnchor(), i.left).setActive(true);
    child.trailingAnchor().constraintEqualToAnchor_constant(&stack.trailingAnchor(), -i.right).setActive(true);
}

pub fn add(stack: &NSStackView, child: &NSView) {
    stack.addArrangedSubview(child);
}

pub fn pin(child: &NSView, parent: &NSView, inset: f64) {
    child.setTranslatesAutoresizingMaskIntoConstraints(false);
    child.leadingAnchor().constraintEqualToAnchor_constant(&parent.leadingAnchor(), inset).setActive(true);
    child.trailingAnchor().constraintEqualToAnchor_constant(&parent.trailingAnchor(), -inset).setActive(true);
    child.topAnchor().constraintEqualToAnchor_constant(&parent.topAnchor(), inset).setActive(true);
    child.bottomAnchor().constraintEqualToAnchor_constant(&parent.bottomAnchor(), -inset).setActive(true);
}

pub fn center_in(child: &NSView, parent: &NSView) {
    child.centerXAnchor().constraintEqualToAnchor(&parent.centerXAnchor()).setActive(true);
    child.centerYAnchor().constraintEqualToAnchor(&parent.centerYAnchor()).setActive(true);
}

pub fn size(v: &NSView, w: f64, h: f64) {
    v.setTranslatesAutoresizingMaskIntoConstraints(false);
    if w > 0.0 {
        v.widthAnchor().constraintEqualToConstant(w).setActive(true);
    }
    if h > 0.0 {
        v.heightAnchor().constraintEqualToConstant(h).setActive(true);
    }
}


pub fn dot(mtm: MainThreadMarker, color: &NSColor, d: f64) -> Retained<NSBox> {
    let b = boxed(mtm, color, d / 2.0);
    size(&b, d, d);
    b
}

/// Legend item: colored dot + caption.
pub fn legend(mtm: MainThreadMarker, color: &NSColor, text: &str) -> Retained<NSStackView> {
    let s = hstack(mtm, 6.0);
    add(&s, &dot(mtm, color, 8.0));
    add(&s, &label(mtm, text, &font(12.0, W::Regular), &Palette::secondary()));
    s
}

/// Small label over a value ("Đã dùng" / "848 GB").
pub fn stat(mtm: MainThreadMarker, caption: &str, value: &str, note: Option<&str>) -> (Retained<NSStackView>, Retained<NSTextField>) {
    let s = vstack(mtm, 3.0);
    add(&s, &label(mtm, caption, &font(12.0, W::Regular), &Palette::secondary()));
    let v = label(mtm, value, &digits(15.0, W::Semibold), &Palette::primary());
    add(&s, &v);
    if let Some(n) = note {
        add(&s, &label(mtm, n, &font(11.0, W::Regular), &Palette::tertiary()));
    }
    (s, v)
}

/// Pill badge.
pub fn badge(mtm: MainThreadMarker, text: &str, fg: &NSColor) -> Retained<NSBox> {
    let b = boxed(mtm, &Palette::tint(fg, 0.16), 9.0);
    let l = label(mtm, text, &font(11.0, W::Semibold), fg);
    l.setContentCompressionResistancePriority_forOrientation(750.0, NSLayoutConstraintOrientation::Horizontal);
    b.addSubview(&l);
    l.leadingAnchor().constraintEqualToAnchor_constant(&b.leadingAnchor(), 9.0).setActive(true);
    l.trailingAnchor().constraintEqualToAnchor_constant(&b.trailingAnchor(), -9.0).setActive(true);
    l.centerYAnchor().constraintEqualToAnchor(&b.centerYAnchor()).setActive(true);
    b.heightAnchor().constraintEqualToConstant(19.0).setActive(true);
    b
}

pub fn separator(mtm: MainThreadMarker) -> Retained<NSBox> {
    let b = boxed(mtm, &Palette::hairline(), 0.0);
    b.heightAnchor().constraintEqualToConstant(1.0).setActive(true);
    b
}

// ---------------------------------------------------------------- controls

pub fn button(mtm: MainThreadMarker, title: &str, target: &Target) -> Retained<NSButton> {
    let b = unsafe { NSButton::buttonWithTitle_target_action(&ns(title), Some(target), Some(fire_sel()), mtm) };
    b.setBezelStyle(NSBezelStyle::Push);
    b.setControlSize(NSControlSize::Large);
    b.setTranslatesAutoresizingMaskIntoConstraints(false);
    b
}

pub fn primary_button(mtm: MainThreadMarker, title: &str, target: &Target) -> Retained<NSButton> {
    let b = button(mtm, title, target);
    b.setBezelColor(Some(&Palette::blue()));
    b
}

pub fn destructive_button(mtm: MainThreadMarker, title: &str, target: &Target) -> Retained<NSButton> {
    let b = button(mtm, title, target);
    b.setHasDestructiveAction(true);
    b.setContentTintColor(Some(&Palette::red()));
    b
}

pub fn with_symbol(b: &NSButton, name: &str) {
    if let Some(img) = symbol(name, 12.0, W::Semibold) {
        b.setImage(Some(&img));
        b.setImagePosition(NSCellImagePosition::ImageLeading);
    }
}

/// Borderless text link like "Xem tất cả ›".
pub fn link_button(mtm: MainThreadMarker, title: &str, color: &NSColor, target: &Target) -> Retained<NSButton> {
    let b = unsafe { NSButton::buttonWithTitle_target_action(&ns(title), Some(target), Some(fire_sel()), mtm) };
    b.setBordered(false);
    b.setContentTintColor(Some(color));
    b.setFont(Some(&font(13.0, W::Semibold)));
    if let Some(img) = symbol("chevron.right", 10.0, W::Bold) {
        b.setImage(Some(&img));
        b.setImagePosition(NSCellImagePosition::ImageTrailing);
    }
    b.setTranslatesAutoresizingMaskIntoConstraints(false);
    b
}

pub fn switch(mtm: MainThreadMarker, on: bool, target: &Target) -> Retained<NSSwitch> {
    let s = NSSwitch::new(mtm);
    s.setState(if on { NSControlStateValueOn } else { NSControlStateValueOff });
    unsafe {
        s.setTarget(Some(target));
        s.setAction(Some(fire_sel()));
    }
    s.setTranslatesAutoresizingMaskIntoConstraints(false);
    s
}

pub fn segmented(mtm: MainThreadMarker, labels: &[&str], selected: isize, target: &Target) -> Retained<NSSegmentedControl> {
    let arr: Vec<Retained<NSString>> = labels.iter().map(|l| ns(l)).collect();
    let arr = NSArray::from_retained_slice(&arr);
    let s = unsafe {
        NSSegmentedControl::segmentedControlWithLabels_trackingMode_target_action(
            &arr,
            NSSegmentSwitchTracking::SelectOne,
            Some(target),
            Some(fire_sel()),
            mtm,
        )
    };
    s.setSelectedSegment(selected);
    s.setTranslatesAutoresizingMaskIntoConstraints(false);
    s
}

pub fn text_field(mtm: MainThreadMarker, value: &str, width: f64) -> Retained<NSTextField> {
    let f = NSTextField::textFieldWithString(&ns(value), mtm);
    f.setAlignment(NSTextAlignment::Right);
    f.setFont(Some(&digits(13.0, W::Regular)));
    f.setTranslatesAutoresizingMaskIntoConstraints(false);
    f.widthAnchor().constraintEqualToConstant(width).setActive(true);
    f
}

// ---------------------------------------------------------------- custom views

define_class!(
    /// Flipped container, used as scroll document view so content starts at the top.
    #[unsafe(super(NSView, NSResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "SDCFlipped"]
    pub struct Flipped;

    impl Flipped {
        #[unsafe(method(isFlipped))]
        fn is_flipped(&self) -> bool {
            true
        }
    }
);

impl Flipped {
    pub fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let v: Retained<Self> = unsafe { msg_send![Self::alloc(mtm), init] };
        v.setTranslatesAutoresizingMaskIntoConstraints(false);
        v
    }
}

/// Vertical scroll view whose document is `content` (a vertical stack), full width.
pub fn scroller(mtm: MainThreadMarker, content: &NSView) -> Retained<NSScrollView> {
    let sv = NSScrollView::new(mtm);
    sv.setHasVerticalScroller(true);
    sv.setAutohidesScrollers(true);
    sv.setDrawsBackground(false);
    sv.setTranslatesAutoresizingMaskIntoConstraints(false);
    let doc = Flipped::new(mtm);
    doc.addSubview(content);
    pin(content, &doc, 0.0);
    sv.setDocumentView(Some(&doc));
    let clip = sv.contentView();
    doc.leadingAnchor().constraintEqualToAnchor(&clip.leadingAnchor()).setActive(true);
    doc.widthAnchor().constraintEqualToAnchor(&clip.widthAnchor()).setActive(true);
    doc.topAnchor().constraintEqualToAnchor(&clip.topAnchor()).setActive(true);
    sv
}

pub struct ClickIvars {
    pub on_click: RefCell<Option<Box<dyn Fn()>>>,
    pub selected: Cell<bool>,
    pub radius: Cell<f64>,
    pub fill: RefCell<Option<Retained<NSColor>>>,
}

define_class!(
    /// Clickable row with a rounded selection background (sidebar items, file rows).
    #[unsafe(super(NSView, NSResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "SDCClickView"]
    #[ivars = ClickIvars]
    pub struct ClickView;

    impl ClickView {
        #[unsafe(method(drawRect:))]
        fn draw_rect(&self, _dirty: NSRect) {
            if self.ivars().selected.get() {
                if let Some(c) = self.ivars().fill.borrow().as_ref() {
                    c.setFill();
                    let r = self.ivars().radius.get();
                    NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(self.bounds(), r, r).fill();
                }
            }
        }

        #[unsafe(method(hitTest:))]
        fn hit_test(&self, point: NSPoint) -> *mut NSView {
            // Let real controls (checkboxes, buttons) inside the row keep their clicks.
            let inner: Option<Retained<NSView>> = unsafe { msg_send![super(self), hitTest: point] };
            match inner {
                Some(v) if v.isKindOfClass(NSControl::class()) && !v.isKindOfClass(NSTextField::class()) => {
                    Retained::as_ptr(&v) as *mut NSView
                }
                Some(_) => self as *const Self as *mut NSView,
                None => std::ptr::null_mut(),
            }
        }

        /// Rows react to the first click even when the window is not key.
        #[unsafe(method(acceptsFirstMouse:))]
        fn accepts_first_mouse(&self, _event: Option<&NSEvent>) -> bool {
            true
        }

        #[unsafe(method(mouseDown:))]
        fn mouse_down(&self, _event: &NSEvent) {
            if let Some(f) = self.ivars().on_click.borrow().as_ref() {
                f();
            }
        }
    }
);

impl ClickView {
    pub fn new(mtm: MainThreadMarker, fill: &NSColor, radius: f64) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(ClickIvars {
            on_click: RefCell::new(None),
            selected: Cell::new(false),
            radius: Cell::new(radius),
            fill: RefCell::new(Some(fill.retain())),
        });
        let v: Retained<Self> = unsafe { msg_send![super(this), initWithFrame: NSRect::ZERO] };
        v.setTranslatesAutoresizingMaskIntoConstraints(false);
        v
    }
    pub fn on_click(&self, f: impl Fn() + 'static) {
        *self.ivars().on_click.borrow_mut() = Some(Box::new(f));
    }
    pub fn set_selected(&self, on: bool) {
        self.ivars().selected.set(on);
        self.setNeedsDisplay(true);
    }
}

pub struct BarIvars {
    /// (fraction start, fraction end, color), drawn in order over the track.
    pub segments: RefCell<Vec<(f64, f64, Retained<NSColor>)>>,
    pub gap: Cell<f64>,
}

define_class!(
    /// Rounded horizontal bar (capacity / split bars).
    #[unsafe(super(NSView, NSResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "SDCBar"]
    #[ivars = BarIvars]
    pub struct Bar;

    impl Bar {
        #[unsafe(method(drawRect:))]
        fn draw_rect(&self, _dirty: NSRect) {
            let b = self.bounds();
            let r = b.size.height / 2.0;
            Palette::track().setFill();
            NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(b, r, r).fill();
            NSGraphicsContext::saveGraphicsState_class();
            NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(b, r, r).addClip();
            let gap = self.ivars().gap.get();
            for (s, e, c) in self.ivars().segments.borrow().iter() {
                let x0 = b.origin.x + b.size.width * s.clamp(0.0, 1.0);
                let x1 = b.origin.x + b.size.width * e.clamp(0.0, 1.0) - gap;
                if x1 <= x0 {
                    continue;
                }
                c.setFill();
                let rect = NSRect::new(NSPoint::new(x0, b.origin.y), NSSize::new(x1 - x0, b.size.height));
                NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(rect, r, r).fill();
            }
            NSGraphicsContext::restoreGraphicsState_class();
        }
    }
);

impl Bar {
    pub fn new(mtm: MainThreadMarker, height: f64, segments: Vec<(f64, f64, Retained<NSColor>)>, gap: f64) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(BarIvars { segments: RefCell::new(segments), gap: Cell::new(gap) });
        let v: Retained<Self> = unsafe { msg_send![super(this), initWithFrame: NSRect::ZERO] };
        v.setTranslatesAutoresizingMaskIntoConstraints(false);
        v.heightAnchor().constraintEqualToConstant(height).setActive(true);
        v
    }
}

pub struct ChartIvars {
    pub samples: RefCell<Vec<Sample>>,
    pub pitch: Cell<f64>,
    pub dot: Cell<f64>,
}

define_class!(
    /// Dot-matrix activity chart: read (cyan) at the bottom, write (orange) stacked above.
    #[unsafe(super(NSView, NSResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "SDCDotChart"]
    #[ivars = ChartIvars]
    pub struct DotChart;

    impl DotChart {
        #[unsafe(method(drawRect:))]
        fn draw_rect(&self, _dirty: NSRect) {
            let b = self.bounds();
            let pitch = self.ivars().pitch.get();
            let d = self.ivars().dot.get();
            let cols = (b.size.width / pitch).floor().max(1.0) as usize;
            let rows = (b.size.height / pitch).floor().max(1.0) as usize;
            let samples = self.ivars().samples.borrow();
            let peak = samples.iter().map(|s| s.read + s.write).fold(0.0f64, f64::max).max(1_000_000.0);
            let x_off = b.size.width - cols as f64 * pitch + (pitch - d) / 2.0;
            let empty = Palette::tint(&Palette::orange(), 0.16);
            let read_c = Palette::cyan();
            let write_c = Palette::orange();
            for c in 0..cols {
                // Right-align samples so "now" sits at the right edge.
                let idx = (c + samples.len()).checked_sub(cols);
                let s = idx.and_then(|i| samples.get(i)).copied().unwrap_or_default();
                let mut rr = (s.read / peak * rows as f64).round() as usize;
                let mut wr = (s.write / peak * rows as f64).round() as usize;
                if s.read > 0.0 && rr == 0 {
                    rr = 1;
                }
                if s.write > 0.0 && wr == 0 {
                    wr = 1;
                }
                if rr + wr > rows {
                    wr = rows.saturating_sub(rr);
                }
                for r in 0..rows {
                    let color = if r < rr { &read_c } else if r < rr + wr { &write_c } else { &empty };
                    color.setFill();
                    let rect = NSRect::new(
                        NSPoint::new(b.origin.x + x_off + c as f64 * pitch, b.origin.y + r as f64 * pitch),
                        NSSize::new(d, d),
                    );
                    NSBezierPath::bezierPathWithOvalInRect(rect).fill();
                }
            }
        }
    }
);

impl DotChart {
    pub fn new(mtm: MainThreadMarker, pitch: f64, dot: f64, height: f64) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(ChartIvars {
            samples: RefCell::new(vec![]),
            pitch: Cell::new(pitch),
            dot: Cell::new(dot),
        });
        let v: Retained<Self> = unsafe { msg_send![super(this), initWithFrame: NSRect::ZERO] };
        v.setTranslatesAutoresizingMaskIntoConstraints(false);
        v.heightAnchor().constraintEqualToConstant(height).setActive(true);
        v
    }
    pub fn columns(&self) -> usize {
        (self.bounds().size.width / self.ivars().pitch.get()).floor().max(24.0) as usize
    }
    pub fn set_samples(&self, s: Vec<Sample>) {
        *self.ivars().samples.borrow_mut() = s;
        self.setNeedsDisplay(true);
    }
}
