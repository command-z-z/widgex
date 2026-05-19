//! macOS renderer implementation.
//!
//! ## 实现说明
//!
//! wry 在 macOS 上通过 `WebViewBuilder::build()` 直接创建独立 NSWindow + WKWebView，
//! 无需 GTK 父窗口。
//!
//! ### 窗口创建
//! ```rust
//! let webview = WebViewBuilder::new()
//!     .with_url("widgex://localhost/index.html")
//!     .with_transparent(true)
//!     .with_custom_protocol("widgex", ...)
//!     .build()?; // macOS 路径：build()，不是 build_gtk()
//! ```
//!
//! ### 桌面层锚定（替代 gtk-layer-shell）
//! 通过 `raw-window-handle` 拿到 NSWindow，然后调用 AppKit：
//! ```
//! raw-window-handle → RawWindowHandle::AppKit(h) → h.ns_window.as_ptr() as id
//!
//! [ns_window setLevel: kCGDesktopWindowLevel]          // Background 层
//! [ns_window setLevel: kCGNormalWindowLevel]            // Bottom 层（Dock 之上）
//! [ns_window setLevel: NSFloatingWindowLevel]           // Top 层
//! [ns_window setLevel: NSScreenSaverWindowLevel]        // Overlay 层
//!
//! [ns_window setCollectionBehavior:
//!     NSWindowCollectionBehaviorCanJoinAllSpaces |
//!     NSWindowCollectionBehaviorStationary |
//!     NSWindowCollectionBehaviorIgnoresCycle]
//! ```
//!
//! ### WindowLayer 映射
//! | widgex Layer | Core Graphics level |
//! |--------------|---------------------|
//! | Background   | `kCGDesktopWindowLevel` (-2147483623) |
//! | Bottom       | `kCGNormalWindowLevel` (0) |
//! | Top          | `NSFloatingWindowLevel` (3) |
//! | Overlay      | `NSScreenSaverWindowLevel` (1000) |
//!
//! ### click_through
//! ```objc
//! [ns_window setIgnoresMouseEvents: YES]
//! ```
//!
//! ### anchor + margin → 坐标计算
//! 使用 `NSScreen.screens[0].visibleFrame`（或 `frame` for background layer）
//! 注意 macOS 坐标系原点在左下角，需转换为左上角坐标系。
//! 逻辑与 `x11_window::compute_xy` 相同，但 y 轴方向相反。
//!
//! ### 控制 socket
//! macOS 是 Unix 系统，可直接复用 `UnixListener` / `UnixStream`（与 Linux 相同）。
//! `run_renderer` 的控制 socket 无需改动。
//!
//! ### 事件循环
//! wry 在 macOS 上使用 `winit` 的 `EventLoop`，或直接调用 `NSApp.run()`。
//! 推荐使用 tao/winit 集成：
//! ```rust
//! use tao::event_loop::{EventLoop, ControlFlow};
//! let event_loop = EventLoop::new();
//! // build webview with event_loop
//! event_loop.run(move |event, _, control_flow| { ... });
//! ```
//!
//! ### 需要添加的依赖（Cargo.toml）
//! ```toml
//! [target.'cfg(target_os = "macos")'.dependencies]
//! objc2 = "0.5"
//! objc2-app-kit = { version = "0.2", features = ["NSWindow", "NSScreen"] }
//! raw-window-handle = "0.6"
//! # 可选：用 tao 管理事件循环（wry 官方推荐）
//! tao = "0.30"
//! ```

use std::path::Path;

use anyhow::Result;
use widgex_core::{Action, DataSource, RendererPayload};

// ── Public API (mirrors linux.rs) ───────────────────────────────────────────
//
// build_window_preview / handle_widget_event / execute_action / inline_theme_css
// 这四个函数不依赖平台 GUI，可直接复用 linux.rs 的实现（提取到 common.rs 后 pub use）。
// macOS 上 `sh -c` 可用，execute_action 基本无需修改。

pub fn build_window_preview(
    _payload: &RendererPayload,
    _window_id: Option<&str>,
) -> Result<crate::WindowPreview> {
    todo!("macOS: build_window_preview — 可直接复用 linux.rs 中的同名函数（无平台依赖）")
}

/// 打开单窗口并运行 macOS 事件循环直到窗口关闭。
///
/// 实现步骤：
/// 1. 调用 `create_webview_window` 创建 WKWebView + NSWindow
/// 2. 调用 `anchor_window` 设置 NSWindow level / collectionBehavior
/// 3. 启动 poll/listen 线程（与 linux.rs 相同）
/// 4. 运行事件循环（tao EventLoop 或 NSApp.run）
/// 5. 在定时器回调中推送 payload 更新（`webview.evaluate_script`）
pub fn run_widget_window(
    _payload: &RendererPayload,
    _config_dir: impl AsRef<Path>,
    _window_id: Option<&str>,
    _sources: &[DataSource],
    _allow_shell: bool,
) -> Result<()> {
    todo!("macOS: run_widget_window — wry build() + NSWindow level 桌面层锚定")
}

/// 多窗口渲染器，通过 UnixListener 接收控制命令（macOS 是 Unix，可复用 Linux 路径）。
///
/// 与 linux.rs 的 `run_renderer` 逻辑几乎相同，只需替换：
/// - `build_gtk(&window)` → `build()`
/// - `gtk::main()` → tao EventLoop / NSApp.run()
/// - `timeout_add_local` → tao EventLoopProxy + 定时器
pub fn run_renderer(
    _payload: &RendererPayload,
    _config_dir: impl AsRef<Path>,
    _config_path: impl AsRef<Path>,
    _sources: &[DataSource],
    _allow_shell: bool,
    _control_socket_path: &Path,
    _initial_window_ids: &[&str],
) -> Result<()> {
    todo!("macOS: run_renderer — UnixListener 控制 socket 可复用，事件循环换用 tao/NSApp")
}

/// 无平台差异，可直接复用 linux.rs 实现。
pub fn handle_widget_event(_body: &str, _config_dir: &Path, _allow_shell: bool) -> Result<()> {
    todo!("macOS: handle_widget_event — 可直接复用 linux.rs 中的同名函数")
}

/// macOS 上 `sh -c` 可用，与 linux.rs 实现相同。
pub fn execute_action(
    _action: &Action,
    _value: Option<&str>,
    _config_dir: &Path,
    _allow_shell: bool,
) -> Result<()> {
    todo!("macOS: execute_action — 可直接复用 linux.rs 中的同名函数（sh -c 在 macOS 可用）")
}

/// 无平台依赖，可直接复用 linux.rs 实现。
pub fn inline_theme_css(_payload: &mut RendererPayload, _config_dir: &Path) {
    todo!("macOS: inline_theme_css — 可直接复用 linux.rs 中的同名函数（无平台依赖）")
}

// ── Internal helpers (stubs) ─────────────────────────────────────────────────

/// 创建 wry WebView（独立 NSWindow）并返回。
///
/// ```
/// WebViewBuilder::new()
///     .with_url(...)
///     .with_transparent(true)
///     .with_custom_protocol("widgex", serve_asset)
///     .with_ipc_handler(...)
///     .build()   // macOS 路径：build()，不是 build_gtk()
/// ```
#[allow(dead_code)]
fn create_webview_window(
    _spec: &widgex_core::RendererWindow,
    _config_dir: &Path,
    _init_script: &str,
    _allow_shell: bool,
) -> Result<wry::WebView> {
    todo!("macOS: create_webview_window")
}

/// 通过 raw-window-handle 拿到 NSWindow，调用 AppKit 完成桌面层锚定。
///
/// ```objc
/// [ns_window setLevel: kCGDesktopWindowLevel];
/// [ns_window setCollectionBehavior:
///     NSWindowCollectionBehaviorCanJoinAllSpaces |
///     NSWindowCollectionBehaviorStationary];
/// [ns_window setIgnoresMouseEvents: click_through];
/// ```
#[allow(dead_code)]
fn anchor_window(_webview: &wry::WebView, _spec: &widgex_core::RendererWindow) {
    todo!("macOS: anchor_window — NSWindow setLevel + setCollectionBehavior")
}

/// 根据 anchor + margin + NSScreen 计算目标坐标。
///
/// 注意 macOS 坐标系原点在屏幕左下角（Quartz 坐标系），
/// 需将计算结果的 y 轴转换：`mac_y = screen_height - widget_y - widget_height`。
#[allow(dead_code)]
fn compute_position(
    _spec: &widgex_core::RendererWindow,
    _monitor_name: Option<&str>,
) -> (f64, f64, f64, f64) {
    // returns (x, y, width, height) in NSScreen (Quartz) coordinates
    todo!("macOS: compute_position — NSScreen.screens + y 轴翻转")
}
