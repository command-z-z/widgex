use gtk::gdk::prelude::*;
use gtk::prelude::*;
use widgex_core::{AnchorEdge, EdgeInsets, SizeSpec, WindowLayer};

const DEFAULT_WIDTH: u32 = 200;
const DEFAULT_HEIGHT: u32 = 200;

/// Call before `show_all()`: sets WM type hint, z-order hints, skip-taskbar/pager,
/// and RGBA visual for transparency. `stick()` is deferred to realize via
/// `connect_realize` because it requires the GDK window to be mapped.
pub(crate) fn apply_hints_before_show(window: &gtk::Window, layer: WindowLayer) {
    window.set_type_hint(map_type_hint(layer));
    window.set_skip_taskbar_hint(true);
    window.set_skip_pager_hint(true);
    match layer {
        WindowLayer::Background => window.set_keep_below(true),
        WindowLayer::Bottom => {}
        WindowLayer::Top | WindowLayer::Overlay => window.set_keep_above(true),
    }
    window.connect_realize(|w| w.stick());

    if let Some(screen) = WidgetExt::screen(window) {
        if let Some(visual) = screen.rgba_visual() {
            WidgetExt::set_visual(window, Some(&visual));
        }
    }
}

/// Call after `show_all()`: positions the window on screen using GDK monitor
/// geometry and the configured anchor + margin values.
pub(crate) fn apply_position_after_show(
    window: &gtk::Window,
    anchor: &[AnchorEdge],
    margin: EdgeInsets,
    size: SizeSpec,
    monitor_name: Option<&str>,
) {
    let monitor = resolve_monitor(window, monitor_name);
    let geo = monitor.geometry();
    let win_w = size.width.unwrap_or(DEFAULT_WIDTH) as i32;
    let win_h = size.height.unwrap_or(DEFAULT_HEIGHT) as i32;

    // All four edges anchored → stretch to fill the monitor minus margins.
    let all_h = anchor.contains(&AnchorEdge::Left) && anchor.contains(&AnchorEdge::Right);
    let all_v = anchor.contains(&AnchorEdge::Top) && anchor.contains(&AnchorEdge::Bottom);
    if all_h && all_v {
        let ml = margin.left as i32;
        let mr = margin.right as i32;
        let mt = margin.top as i32;
        let mb = margin.bottom as i32;
        window.resize(geo.width() - ml - mr, geo.height() - mt - mb);
        window.move_(geo.x() + ml, geo.y() + mt);
        return;
    }

    let (x, y) = compute_xy(anchor, margin, &geo, win_w, win_h);
    window.move_(x, y);
}

fn map_type_hint(layer: WindowLayer) -> gtk::gdk::WindowTypeHint {
    match layer {
        WindowLayer::Background => gtk::gdk::WindowTypeHint::Desktop,
        WindowLayer::Bottom => gtk::gdk::WindowTypeHint::Dock,
        WindowLayer::Top => gtk::gdk::WindowTypeHint::Notification,
        WindowLayer::Overlay => gtk::gdk::WindowTypeHint::Splashscreen,
    }
}

fn resolve_monitor(_window: &gtk::Window, name: Option<&str>) -> gtk::gdk::Monitor {
    let display = gtk::gdk::Display::default().expect("GDK display");
    let n = display.n_monitors();
    if let Some(name) = name {
        for i in 0..n {
            if let Some(mon) = display.monitor(i) {
                if mon.model().as_deref() == Some(name) {
                    return mon;
                }
            }
        }
        eprintln!("widgex x11: monitor {name:?} not found, falling back to primary");
    }
    display
        .primary_monitor()
        .or_else(|| display.monitor(0))
        .expect("at least one monitor")
}

fn compute_xy(
    anchor: &[AnchorEdge],
    margin: EdgeInsets,
    geo: &gtk::gdk::Rectangle,
    win_w: i32,
    win_h: i32,
) -> (i32, i32) {
    let has = |e: AnchorEdge| anchor.contains(&e);

    let x = match (has(AnchorEdge::Left), has(AnchorEdge::Right)) {
        (true, false) => geo.x() + margin.left as i32,
        (false, true) => geo.x() + geo.width() - win_w - margin.right as i32,
        (true, true) => geo.x() + margin.left as i32,
        (false, false) => geo.x() + (geo.width() - win_w) / 2,
    };
    let y = match (has(AnchorEdge::Top), has(AnchorEdge::Bottom)) {
        (true, false) => geo.y() + margin.top as i32,
        (false, true) => geo.y() + geo.height() - win_h - margin.bottom as i32,
        (true, true) => geo.y() + margin.top as i32,
        (false, false) => geo.y() + (geo.height() - win_h) / 2,
    };
    (x, y)
}
