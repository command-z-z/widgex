//! Native GTK renderer for transparent layer-shell windows.
//!
//! webkit2gtk on Hyprland (wry#1524) leaves ghost pixels in transparent
//! windows due to DMA-BUF damage-region optimisation.  This renderer replaces
//! the WebView with a GTK widget tree (GtkBox/GtkLabel/GtkDrawingArea) that
//! uses wl_shm software rendering — GTK repaints the entire surface on every
//! `queue_draw()` call, so no ghost pixels are left behind.
//!
//! Remove this module (and the `native_render = true` flags in configs) once
//! the upstream webkit2gtk / wry issue is fixed.

use std::{cell::RefCell, collections::HashMap, path::Path, rc::Rc, time::Duration};

use gtk::{
    cairo,
    pango,
    prelude::*,
};
use gtk_layer_shell::{Edge, KeyboardMode, LayerShell};
use widgex_core::{AnchorEdge, Direction, RendererWidget, RendererWindow, WidgetKind};

use crate::{map_edge, map_layer};

// ── Sprite animation state ───────────────────────────────────────────────────

struct SpriteState {
    /// Full spritesheet as a Cairo surface (pre-converted from Pixbuf once).
    sheet: cairo::ImageSurface,
    frame_width: i32,
    frame_height: i32,
    cols: i32,
    x: i32,
    y: i32,
    row: i32,
    frame: i32,
    frame_count: i32,
    frame_durations: Vec<u32>,
    elapsed_ms: u32,
}

struct SpriteHandle {
    drawing_area: gtk::DrawingArea,
    state: Rc<RefCell<SpriteState>>,
    // Held to keep the animation timer alive (glib::SourceId does not
    // auto-cancel on drop in glib 0.18, but keeping it named with _ prefix
    // documents intent and satisfies lint).
    _timer: gtk::glib::SourceId,
}

impl SpriteHandle {
    fn new(widget: &RendererWidget, config_dir: &Path) -> Option<Self> {
        let src = widget.src.as_deref()?;
        let file_path = config_dir.join(src);
        let pixbuf = gdk_pixbuf::Pixbuf::from_file(&file_path)
            .map_err(|e| {
                eprintln!(
                    "widgex native: failed to load sprite {}: {e}",
                    file_path.display()
                )
            })
            .ok()?;

        let sheet = pixbuf_to_cairo_surface(&pixbuf)?;

        let fw = widget.frame_width.unwrap_or(192) as i32;
        let fh = widget.frame_height.unwrap_or(208) as i32;
        let cols = widget.cols.unwrap_or(1) as i32;
        let durations = if widget.frame_durations.is_empty() {
            vec![150u32]
        } else {
            widget.frame_durations.clone()
        };

        let state = Rc::new(RefCell::new(SpriteState {
            sheet,
            frame_width: fw,
            frame_height: fh,
            cols,
            x: 0,
            y: 0,
            row: 0,
            frame: 0,
            frame_count: 1,
            frame_durations: durations,
            elapsed_ms: 0,
        }));

        let drawing_area = gtk::DrawingArea::new();
        drawing_area.set_hexpand(true);
        drawing_area.set_vexpand(true);

        // Paint sprite frame via Cairo
        let state_draw = Rc::clone(&state);
        drawing_area.connect_draw(move |_da, ctx| {
            let s = state_draw.borrow();

            // Erase previous content (Source operator replaces all pixels)
            ctx.set_operator(cairo::Operator::Source);
            ctx.set_source_rgba(0.0, 0.0, 0.0, 0.0);
            ctx.paint().ok();
            ctx.set_operator(cairo::Operator::Over);

            // Clip to the sprite frame region at the destination position
            ctx.rectangle(
                s.x as f64,
                s.y as f64,
                s.frame_width as f64,
                s.frame_height as f64,
            );
            ctx.clip();

            // Set source to the spritesheet surface, offset so the correct
            // frame tile is visible at the destination position.
            let tile_x = (s.frame % s.cols) * s.frame_width;
            let tile_y = s.row * s.frame_height;
            ctx.set_source_surface(
                &s.sheet,
                (s.x - tile_x) as f64,
                (s.y - tile_y) as f64,
            )
            .ok();
            ctx.paint().ok();

            gtk::glib::Propagation::Proceed
        });

        // Animation timer: 16 ms fixed tick, frame advance tracked via elapsed time
        let state_timer = Rc::clone(&state);
        let da_timer = drawing_area.clone();
        let timer = gtk::glib::timeout_add_local(Duration::from_millis(16), move || {
            let mut s = state_timer.borrow_mut();
            s.elapsed_ms += 16;
            let dur = s
                .frame_durations
                .get(s.frame as usize)
                .copied()
                .unwrap_or(150);
            if s.elapsed_ms >= dur {
                s.elapsed_ms -= dur;
                s.frame = (s.frame + 1) % s.frame_count.max(1);
            }
            drop(s);
            da_timer.queue_draw();
            gtk::glib::ControlFlow::Continue
        });

        Some(SpriteHandle {
            drawing_area,
            state,
            _timer: timer,
        })
    }

    fn update_state(&self, x: i32, y: i32, row: i32, frame_count: i32) {
        let mut s = self.state.borrow_mut();
        s.x = x;
        s.y = y;
        s.row = row;
        s.frame_count = frame_count;
    }
}

/// Convert a GDK Pixbuf to a Cairo ARgb32 ImageSurface (once at load time).
/// Cairo uses premultiplied BGRA; GDK Pixbuf uses straight RGBA.
fn pixbuf_to_cairo_surface(pb: &gdk_pixbuf::Pixbuf) -> Option<cairo::ImageSurface> {
    let w = pb.width();
    let h = pb.height();
    let has_alpha = pb.has_alpha();
    let rowstride = pb.rowstride() as usize;
    let bpp: usize = if has_alpha { 4 } else { 3 };
    let pixels = unsafe { pb.pixels() };

    let mut surface = cairo::ImageSurface::create(cairo::Format::ARgb32, w, h).ok()?;
    let stride = surface.stride() as usize;

    {
        let mut data = surface.data().ok()?;
        for row in 0..h as usize {
            let src_row = row * rowstride;
            let dst_row = row * stride;
            for col in 0..w as usize {
                let s = src_row + col * bpp;
                let d = dst_row + col * 4;
                let r = pixels[s];
                let g = pixels[s + 1];
                let b = pixels[s + 2];
                let a = if has_alpha { pixels[s + 3] } else { 255u8 };
                // Cairo ARgb32 (little-endian): B G R A, premultiplied
                let af = a as f32 / 255.0;
                data[d]     = (b as f32 * af) as u8;
                data[d + 1] = (g as f32 * af) as u8;
                data[d + 2] = (r as f32 * af) as u8;
                data[d + 3] = a;
            }
        }
    }

    Some(surface)
}

// ── NativeRenderer ───────────────────────────────────────────────────────────

/// Renders a `RendererWindow` via GTK native widgets instead of webkit.
/// Ghost-free on transparent Hyprland layer-shell windows (wl_shm rendering,
/// no DMA-BUF damage regions).
pub(crate) struct NativeRenderer {
    pub(crate) window: gtk::Window,
    // Labels collected in DFS widget-tree order, updated each tick.
    labels: Vec<gtk::Label>,
    sprites: Vec<SpriteHandle>,
}

impl NativeRenderer {
    pub(crate) fn new(
        win_spec: &RendererWindow,
        config_dir: &Path,
        // Full concatenated CSS (theme + widget module CSS), already inlined.
        all_css: Option<&str>,
    ) -> Option<Self> {
        // ── GTK window + layer shell ─────────────────────────────────────────
        let window = gtk::Window::new(gtk::WindowType::Toplevel);
        window.set_app_paintable(true);
        window.set_decorated(false);
        window.set_title(win_spec.title.as_deref().unwrap_or(&win_spec.id));

        // RGBA visual → transparent window
        if let Some(screen) = gtk::prelude::WidgetExt::screen(&window) {
            if let Some(visual) = screen.rgba_visual() {
                gtk::prelude::WidgetExt::set_visual(&window, Some(&visual));
            }
        }

        // Fixed size (same logic as the webkit path)
        let w = win_spec.size.width.map_or(-1, |v| v as i32);
        let h = win_spec.size.height.map_or(-1, |v| v as i32);
        window.set_size_request(w, h);
        window.set_default_size(
            win_spec.size.width.unwrap_or(200) as i32,
            win_spec.size.height.unwrap_or(200) as i32,
        );

        window.init_layer_shell();
        window.set_namespace("widgex-native");
        window.set_layer(map_layer(win_spec.layer));
        window.set_keyboard_mode(KeyboardMode::None);

        for edge in [AnchorEdge::Top, AnchorEdge::Right, AnchorEdge::Bottom, AnchorEdge::Left] {
            window.set_anchor(map_edge(edge), win_spec.anchor.contains(&edge));
        }
        window.set_layer_shell_margin(Edge::Top, win_spec.margin.top as i32);
        window.set_layer_shell_margin(Edge::Right, win_spec.margin.right as i32);
        window.set_layer_shell_margin(Edge::Bottom, win_spec.margin.bottom as i32);
        window.set_layer_shell_margin(Edge::Left, win_spec.margin.left as i32);
        window.set_exclusive_zone(win_spec.exclusive_zone.unwrap_or(0));

        if win_spec.click_through {
            let region = cairo::Region::create();
            window.input_shape_combine_region(Some(&region));
        }

        // ── CSS provider ────────────────────────────────────────────────────
        // Apply the widget CSS to the GTK widget tree.  GTK3 CSS supports:
        // color, background-color, font-size, font-weight, padding, margin,
        // border-radius — sufficient for labels and containers.
        // CSS variables (var(--ctp-*)) are resolved before loading.
        if let Some(css) = all_css {
            let resolved = strip_unsupported_props(&resolve_css_vars(css));
            let provider = gtk::CssProvider::new();
            if let Err(e) = provider.load_from_data(resolved.as_bytes()) {
                eprintln!("widgex native: CSS load warning: {e}");
            }
            if let Some(screen) = gtk::gdk::Screen::default() {
                gtk::StyleContext::add_provider_for_screen(
                    &screen,
                    &provider,
                    gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
                );
            }
        }

        // ── Widget tree ──────────────────────────────────────────────────────
        let mut labels: Vec<gtk::Label> = Vec::new();
        let mut sprites: Vec<SpriteHandle> = Vec::new();

        if let Some(root) = build_gtk_tree(&win_spec.widgets, config_dir, &mut labels, &mut sprites) {
            window.add(&root);
        }

        window.show_all();

        Some(NativeRenderer { window, labels, sprites })
    }

    /// Called on each tick with the resolved widget tree (source templates already
    /// substituted).  Updates label text and sprite position/frame-count.
    pub(crate) fn update(&self, win: &RendererWindow) {
        let mut label_iter = self.labels.iter();
        let mut sprite_iter = self.sprites.iter();
        update_dfs(&win.widgets, &mut label_iter, &mut sprite_iter);
    }
}

// ── Widget tree builder ──────────────────────────────────────────────────────

/// Recursively build GTK widgets from the `RendererWidget` tree.
/// Labels and sprites are appended in DFS order to the output vecs —
/// `update_dfs` walks the same order so indices stay consistent.
fn build_gtk_tree(
    widgets: &[RendererWidget],
    config_dir: &Path,
    labels: &mut Vec<gtk::Label>,
    sprites: &mut Vec<SpriteHandle>,
) -> Option<gtk::Widget> {
    if widgets.is_empty() {
        return None;
    }
    if widgets.len() == 1 {
        return build_single(widgets, 0, config_dir, labels, sprites);
    }
    // Multiple top-level widgets → wrap in a vertical GtkBox
    let container = gtk::Box::new(gtk::Orientation::Vertical, 0);
    for i in 0..widgets.len() {
        if let Some(child) = build_single(widgets, i, config_dir, labels, sprites) {
            container.pack_start(&child, false, true, 0);
        }
    }
    Some(container.upcast())
}

fn build_single(
    widgets: &[RendererWidget],
    idx: usize,
    config_dir: &Path,
    labels: &mut Vec<gtk::Label>,
    sprites: &mut Vec<SpriteHandle>,
) -> Option<gtk::Widget> {
    let w = &widgets[idx];
    match w.kind {
        WidgetKind::Box => {
            let orient = match w.direction {
                Some(Direction::Row) => gtk::Orientation::Horizontal,
                _ => gtk::Orientation::Vertical,
            };
            let gbox = gtk::Box::new(orient, 0);
            apply_classes(&gbox, &w.class);
            for i in 0..w.children.len() {
                if let Some(child) = build_single(&w.children, i, config_dir, labels, sprites) {
                    gbox.pack_start(&child, false, true, 0);
                }
            }
            Some(gbox.upcast())
        }

        WidgetKind::Label => {
            let label = gtk::Label::new(w.text.as_deref());
            label.set_ellipsize(pango::EllipsizeMode::End);
            label.set_xalign(0.5);
            label.set_hexpand(true);
            apply_classes(&label, &w.class);
            labels.push(label.clone());
            Some(label.upcast())
        }

        WidgetKind::Animation if w.draw_x.is_some() && w.draw_y.is_some() => {
            if let Some(sprite) = SpriteHandle::new(w, config_dir) {
                let da = sprite.drawing_area.clone();
                sprites.push(sprite);
                Some(da.upcast())
            } else {
                None
            }
        }

        WidgetKind::Spacer => {
            let spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
            spacer.set_hexpand(true);
            spacer.set_vexpand(true);
            apply_classes(&spacer, &w.class);
            Some(spacer.upcast())
        }

        WidgetKind::Image => {
            if let Some(src) = &w.src {
                let path = config_dir.join(src);
                let img = match gdk_pixbuf::Pixbuf::from_file(&path) {
                    Ok(pb) => gtk::Image::from_pixbuf(Some(&pb)),
                    Err(_) => gtk::Image::new(),
                };
                apply_classes(&img, &w.class);
                Some(img.upcast())
            } else {
                None
            }
        }

        _ => None, // button/progress not yet supported in native renderer
    }
}

// ── Update (called each tick) ────────────────────────────────────────────────

fn update_dfs<'a>(
    widgets: &[RendererWidget],
    labels: &mut std::slice::Iter<'a, gtk::Label>,
    sprites: &mut std::slice::Iter<'a, SpriteHandle>,
) {
    for w in widgets {
        match w.kind {
            WidgetKind::Label => {
                if let Some(label) = labels.next() {
                    let text = w.text.as_deref().unwrap_or("");
                    if label.text() != text {
                        label.set_text(text);
                    }
                }
            }
            WidgetKind::Animation if w.draw_x.is_some() && w.draw_y.is_some() => {
                if let Some(sprite) = sprites.next() {
                    let x: i32 = w.draw_x.as_deref().unwrap_or("0").parse().unwrap_or(0);
                    let y: i32 = w.draw_y.as_deref().unwrap_or("0").parse().unwrap_or(0);
                    let row: i32 = w.frame_row.as_deref().unwrap_or("0").parse().unwrap_or(0);
                    let fc: i32 = w.frame_count.as_deref().unwrap_or("1").parse().unwrap_or(1);
                    sprite.update_state(x, y, row, fc);
                }
            }
            WidgetKind::Box => {
                update_dfs(&w.children, labels, sprites);
            }
            _ => {}
        }
    }
}

// ── CSS helpers ──────────────────────────────────────────────────────────────

fn apply_classes<W: IsA<gtk::Widget>>(widget: &W, classes: &[String]) {
    let ctx = widget.style_context();
    for class in classes {
        ctx.add_class(class.as_str());
    }
}

/// Strip CSS properties GTK3's engine doesn't support.
/// GTK3 skips the ENTIRE rule block when it encounters an unknown property,
/// so we must remove unsupported declarations before loading.
fn strip_unsupported_props(css: &str) -> String {
    // GTK3 CSS subset: supports color, background-color, font-*, padding,
    // margin, border, border-radius, opacity, box-shadow, min-width/height.
    // Everything below causes GTK to silently skip the whole rule.
    const UNSUPPORTED: &[&str] = &[
        "overflow",
        "text-overflow",
        "white-space",
        "display",
        "line-height",
        "text-align",
        "max-width",
        "max-height",
        "align-items",
        "justify-content",
        "flex",
    ];
    css.lines()
        .filter(|line| {
            let t = line.trim();
            !UNSUPPORTED.iter().any(|prop| {
                t.starts_with(prop)
                    && t[prop.len()..].trim_start().starts_with(':')
            })
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Resolve CSS custom properties (`var(--name)`) defined in a `:root` block.
/// GTK3's CssProvider doesn't understand `var(--x)`, so we inline values.
fn resolve_css_vars(css: &str) -> String {
    let vars = parse_css_vars(css);
    if vars.is_empty() {
        return css.to_string();
    }
    let mut result = css.to_string();
    for (name, value) in &vars {
        let placeholder = format!("var(--{})", name);
        result = result.replace(&placeholder, value);
    }
    result
}

fn parse_css_vars(css: &str) -> HashMap<String, String> {
    let mut vars = HashMap::new();
    let Some(root_pos) = css.find(":root") else {
        return vars;
    };
    let after_root = &css[root_pos..];
    let Some(open) = after_root.find('{') else {
        return vars;
    };
    let block_start = root_pos + open + 1;
    let Some(close) = css[block_start..].find('}') else {
        return vars;
    };
    let block = &css[block_start..block_start + close];
    for line in block.lines() {
        let line = line.trim();
        let Some(rest) = line.strip_prefix("--") else {
            continue;
        };
        let Some(colon) = rest.find(':') else {
            continue;
        };
        let name = rest[..colon].trim().to_string();
        let value = rest[colon + 1..]
            .trim()
            .trim_end_matches(';')
            .trim()
            .to_string();
        if !name.is_empty() && !value.is_empty() {
            vars.insert(name, value);
        }
    }
    vars
}
