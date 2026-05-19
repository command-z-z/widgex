//! Webview-backed widget window.
//!
//! This file is a thin platform dispatcher. Each OS gets its own implementation
//! module; the public API surface is identical across all platforms.
//!
//! | Platform        | Module          | Backend |
//! |-----------------|-----------------|---------|
//! | Linux/Wayland   | `linux`         | GTK + gtk-layer-shell + webkit2gtk |
//! | Linux/X11       | `linux`         | GTK + EWMH hints + webkit2gtk |
//! | Windows         | `windows`       | wry/WebView2 + Win32 SetWindowPos |
//! | macOS           | `macos`         | wry/WKWebView + NSWindow level |
//! | Other           | `stub`          | bail!() stubs |

/// A non-GUI summary of a window, used by `widgex open --dry-run`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowPreview {
    pub id: String,
    pub title: Option<String>,
    pub width: u32,
    pub height: u32,
    pub text_preview: String,
}

// ── Linux ────────────────────────────────────────────────────────────────────

#[cfg(target_os = "linux")]
pub(crate) mod linux;
#[cfg(target_os = "linux")]
pub(crate) mod native_renderer;
#[cfg(target_os = "linux")]
mod x11_window;

#[cfg(target_os = "linux")]
pub use linux::{
    build_window_preview, execute_action, handle_widget_event, inline_theme_css, run_renderer,
    run_widget_window,
};

// ── Windows ──────────────────────────────────────────────────────────────────

#[cfg(target_os = "windows")]
mod windows;
#[cfg(target_os = "windows")]
pub use windows::{
    build_window_preview, execute_action, handle_widget_event, inline_theme_css, run_renderer,
    run_widget_window,
};

// ── macOS ────────────────────────────────────────────────────────────────────

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
pub use macos::{
    build_window_preview, execute_action, handle_widget_event, inline_theme_css, run_renderer,
    run_widget_window,
};

// ── Other platforms (FreeBSD, etc.) ─────────────────────────────────────────

#[cfg(not(any(target_os = "linux", target_os = "windows", target_os = "macos")))]
mod stub;
#[cfg(not(any(target_os = "linux", target_os = "windows", target_os = "macos")))]
pub use stub::*;
