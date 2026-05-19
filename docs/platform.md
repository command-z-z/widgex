# Platform Support

本文档描述 widgex 在各平台的支持状态、构建要求和已知限制。

---

## 支持状态总览

| 平台 | 状态 | 稳定性 |
|------|------|--------|
| Linux / Wayland | ✅ 完整支持 | 生产可用（主要开发目标） |
| Linux / X11 | ✅ 完整支持 | 可用，`exclusive_zone` 无效 |
| macOS | 🚧 接口已定义 | 不可用，待实现 |
| Windows | 🚧 接口已定义 | 不可用，待实现 |

---

## Linux / Wayland

### 构建依赖

| 包 | 用途 |
|----|------|
| `gtk3` | 窗口系统 |
| `gtk-layer-shell` | `zwlr_layer_shell_v1` 协议绑定 |
| `webkit2gtk-4.1` | WebKit 渲染引擎 |
| `gdk-pixbuf-2.0` | 图像加载（精灵图） |
| `pkg-config` | 构建时库发现 |

Arch Linux:
```bash
sudo pacman -S gtk3 gtk-layer-shell webkit2gtk-4.1 gdk-pixbuf2 pkg-config
```

Ubuntu 24.04+:
```bash
sudo apt install libgtk-3-dev libgtk-layer-shell-dev \
    libwebkit2gtk-4.1-dev libgdk-pixbuf-2.0-dev pkg-config
```

### 前提条件

- 运行 Wayland 合成器（Hyprland、Sway、GNOME on Wayland 等）
- 合成器支持 `zwlr_layer_shell_v1` 协议
- `$WAYLAND_DISPLAY` 环境变量已设置

### 会话类型检测

```bash
widgex doctor
```

输出示例：
```
session type : wayland
WAYLAND_DISPLAY : wayland-1
config path : /home/user/.config/widgex/widgex/config.toml
```

### 快速开始

```bash
# 安装
cargo install --path crates/widgex-cli

# 初始化配置
widgex init --template top-bar

# 启动 daemon
widgex daemon start

# 打开 widget
widgex open bar

# 后台常驻（加入 hyprland.conf）
exec-once = widgexd
exec-once = widgex open bar
```

### layer 与 anchor 配置

```toml
[[windows]]
id = "bar"
layer = "top"            # background | bottom | top | overlay
anchor = ["top", "left", "right"]
click_through = false

[windows.margin]
top = 0

[windows.size]
height = 32

exclusive_zone = 32      # 向其他层保留 32px 空间（推开其他窗口）
```

**layer 说明：**

| layer | 行为 |
|-------|------|
| `background` | 桌面壁纸层，在所有普通窗口之下 |
| `bottom` | 桌面层，在普通窗口之下但在壁纸之上 |
| `top` | 在普通窗口之上（状态栏常用） |
| `overlay` | 最高层，覆盖全屏应用 |

### native_render 标志（透明窗口 ghosting 修复）

webkit2gtk 在透明的 layer-shell 窗口上存在 ghost pixel 问题（wry#1524）。
受影响的窗口（通常是含动画的透明 widget）请开启此标志：

```toml
[[windows]]
id = "pet"
native_render = true     # 使用 GTK/Cairo 软件渲染替代 WebKit
```

**局限**：`native_render = true` 仅支持 `label`、`image`、`animation`、`box`、`spacer` 类型。
`button`、`progress`、`canvas` 在此模式下不渲染。

### 已知限制

- `exclusive_zone` 在 GNOME on Wayland 下可能与 GNOME Shell 的保留区域冲突。
- 在 NVIDIA 专有驱动 + wlroots 下，DMA-BUF 路径可能引发黑屏；设置
  `WEBKIT_DISABLE_DMABUF_RENDERER=1`（widgex 已自动设置）。

---

## Linux / X11

### 额外说明

X11 使用与 Wayland 相同的二进制，在运行时自动检测：

```
gtk_layer_shell::is_supported() == false  →  X11 路径
```

无需特殊构建参数。只要安装了同样的 GTK3 + webkit2gtk 依赖即可。

### 前提条件

- 运行 X11 会话（i3、Openbox、Xfce 等）或 XWayland（不推荐，详见下文）
- `$DISPLAY` 环境变量已设置

### 快速开始

与 Wayland 完全相同：

```bash
widgex daemon start
widgex open bar
```

### layer 映射

X11 没有 layer-shell 协议，widgex 使用 EWMH/ICCCM 窗口提示替代：

| layer | `_NET_WM_WINDOW_TYPE` | z-order |
|-------|-----------------------|---------|
| `background` | `_NET_WM_WINDOW_TYPE_DESKTOP` | `keep_below` |
| `bottom` | `_NET_WM_WINDOW_TYPE_DOCK` | WM 管理 |
| `top` | `_NET_WM_WINDOW_TYPE_NOTIFICATION` | `keep_above` |
| `overlay` | `_NET_WM_WINDOW_TYPE_SPLASH` | `keep_above` |

所有窗口均自动设置 `skip_taskbar`、`skip_pager`，并在所有虚拟桌面上显示（`stick()`）。

### anchor + margin 行为

anchor 决定窗口固定到哪条边，margin 是对应边的像素偏移量：

```toml
anchor = ["top", "right"]      # 右上角
[windows.margin]
top = 10
right = 10
```

同时锚定对立边（如 `left` + `right`）时，窗口自动拉伸填充该轴并减去 margin：

```toml
anchor = ["top", "left", "right"]   # 顶部全宽横幅
```

**monitor 选择：**

```toml
monitor = "HDMI-1"    # 按连接器名称选择显示器
```

名称不匹配时回退到 primary monitor（`gdk::Display::primary_monitor()`）。

### 已知限制

- `exclusive_zone` 在 X11 下无效（需要 `_NET_WM_STRUT_PARTIAL`，暂未实现），值会被忽略。
- 部分非 EWMH 合规的 WM（如 Openbox 默认配置）可能不遵守 `keep_above/keep_below` 提示。
- 在 XWayland 下运行时，`gtk_layer_shell::is_supported()` 返回 `false`，
  走 X11 路径，但实际显示效果取决于 XWayland 的合成器支持，**不推荐**。

---

## macOS

> **状态：接口已定义，实现待完成。**
>
> `src/macos.rs` 中的所有函数均为 `todo!()` 占位符。
> 编译可通过，运行时会 panic。

### 计划实现方案

- **渲染引擎**：wry 0.55 → WKWebView（系统自带，无需额外依赖）
- **窗口管理**：`wry::WebViewBuilder::build()` 创建独立 NSWindow，
  通过 `raw-window-handle` 获取 NSWindow 指针后调用 AppKit
- **桌面层锚定**：`[NSWindow setLevel:]` + `[NSWindow setCollectionBehavior:]`
- **点击穿透**：`[NSWindow setIgnoresMouseEvents: YES]`
- **控制 socket**：macOS 是 Unix，可直接复用 `UnixListener`（与 Linux 相同）
- **事件循环**：tao `EventLoop` 或 `NSApp.run()`

### layer → NSWindow level 映射（计划）

| layer | NSWindow level |
|-------|---------------|
| `background` | `kCGDesktopWindowLevel` (-2147483623) |
| `bottom` | `kCGNormalWindowLevel` (0) |
| `top` | `NSFloatingWindowLevel` (3) |
| `overlay` | `NSScreenSaverWindowLevel` (1000) |

### macOS 特有注意事项

- **坐标系**：macOS 的 Quartz 坐标原点在屏幕左下角，y 轴向上。
  widgex 的 `anchor = ["top"]` 对应 mac y 坐标 = `screen_height - widget_height - margin_top`。
- **多显示器**：使用 `NSScreen.screens` 按 `localizedName` 匹配 `monitor` 配置项。
- **透明度**：WKWebView 本身支持透明，不存在 Linux 的 ghosting 问题。
  `native_render` 标志在 macOS 上无意义（始终使用 WebKit 路径）。
- **权限**：`execute_action` 在 macOS 上可直接使用 `sh -c`（与 Linux 相同）。

### 需要新增的依赖

```toml
[target.'cfg(target_os = "macos")'.dependencies]
objc2 = "0.5"
objc2-app-kit = { version = "0.2", features = ["NSWindow", "NSScreen"] }
raw-window-handle = "0.6"
tao = "0.30"   # 可选，用于事件循环管理
```

### 实现入口

`crates/widgex-webview/src/macos.rs` — 所有函数均有详细注释说明实现步骤。

---

## Windows

> **状态：接口已定义，实现待完成。**
>
> `src/windows.rs` 中的所有函数均为 `todo!()` 占位符。
> 编译可通过，运行时会 panic。

### 计划实现方案

- **渲染引擎**：wry 0.55 → WebView2（Edge Chromium，需要用户安装 WebView2 Runtime）
- **窗口管理**：`wry::WebViewBuilder::build()` 创建独立 HWND，
  通过 `raw-window-handle` 获取 HWND 后调用 Win32 API
- **桌面层锚定**：`SetWindowPos` + z-order 常量
- **点击穿透**：`SetWindowLongW(WS_EX_TRANSPARENT | WS_EX_LAYERED)`
- **控制 socket**：Named Pipe（`\\.\pipe\widgex-renderer`），替代 `UnixListener`

### layer → Win32 z-order 映射（计划）

| layer | HWND z-order | 额外样式 |
|-------|-------------|---------|
| `background` | `HWND_BOTTOM` + `SetParent(GetDesktopWindow())` | — |
| `bottom` | `HWND_BOTTOM` | — |
| `top` | `HWND_TOPMOST` | — |
| `overlay` | `HWND_TOPMOST` | 最前置 |

所有窗口均设置 `WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE`（不显示在任务栏，不抢焦点）。

### Windows 特有注意事项

- **WebView2 Runtime**：用户须预装 WebView2 Runtime（Edge 现代版本自带，
  或从 Microsoft 官网独立下载）。
- **shell 命令**：`execute_action` 需要将 `sh -c <cmd>` 替换为 `cmd /c <cmd>`
  （或 PowerShell），这是与 Unix 实现的唯一语义差异。
- **unix_socket 数据源**：Windows 无 Unix domain socket，
  `source.kind = "unix_socket"` 在 Windows 下无效，使用此数据源的 widget 将无法正常工作。
- **IPC 传输**：daemon socket 和 renderer socket 均使用 Named Pipe，
  路径格式从 `/run/user/1000/widgex.sock` 变为 `\\.\pipe\widgex`。
- **进程组**：Windows 无 `process_group(0)` / `killpg`，
  计划使用 Job Object 确保渲染器子进程随 daemon 退出。
- **透明度**：WebView2 支持透明窗口，不存在 ghosting 问题。`native_render` 在 Windows 无意义。

### 需要新增的依赖

```toml
[target.'cfg(target_os = "windows")'.dependencies]
windows = { version = "0.58", features = [
    "Win32_UI_WindowsAndMessaging",
    "Win32_Graphics_Gdi",
    "Win32_System_Pipes",
    "Win32_System_Threading",
] }
raw-window-handle = "0.6"
```

### 实现入口

`crates/widgex-webview/src/windows.rs` — 所有函数均有详细注释说明实现步骤。

`crates/widgex-ipc/src/lib.rs` — `#[cfg(windows)]` 分支中的 `send_request` /
`send_renderer_request` 需要实现 Named Pipe 客户端。

`crates/widgexd/src/lib.rs` — `#[cfg(windows)]` 分支中的 `run_socket_daemon`
需要实现 Named Pipe 服务端循环。

---

## 跨平台配置注意事项

大多数配置项在所有平台上语义相同，以下是例外：

| 配置项 | Linux/Wayland | Linux/X11 | macOS | Windows |
|--------|--------------|-----------|-------|---------|
| `layer` | 完整支持 | 近似映射（EWMH） | 计划中 | 计划中 |
| `anchor` | 完整支持 | 完整支持 | 计划中 | 计划中 |
| `exclusive_zone` | 完整支持 | **无效（忽略）** | 计划中 | 计划中 |
| `native_render` | 有效（ghosting 修复） | 有效 | **无意义（忽略）** | **无意义（忽略）** |
| `monitor` | 按连接器名称 | 按 GDK model 名称 | 计划中 | 计划中 |
| `unix_socket` 数据源 | 完整支持 | 完整支持 | 完整支持 | **不支持** |
| `battery` 数据源 | 完整支持 | 完整支持 | **空（无 sysfs）** | **空（无 sysfs）** |
| `shell` 命令 | `sh -c` | `sh -c` | `sh -c` | 需 `cmd /c`（todo） |

---

## 为新平台贡献实现

1. **渲染器**：填充 `crates/widgex-webview/src/<platform>.rs`，
   参照文件内注释实现所有 `todo!()` 函数。
2. **IPC 传输**（仅 Windows）：实现 `widgex-ipc/src/lib.rs` 中
   `#[cfg(windows)]` 块里的 `send_request` / `send_renderer_request`。
3. **Daemon**（仅 Windows）：实现 `widgexd/src/lib.rs` 中
   `#[cfg(windows)]` 块里的 `run_socket_daemon`。
4. **Cargo.toml**：在 `crates/widgex-webview/Cargo.toml` 的对应
   `[target.'cfg(...)'.dependencies]` 块中添加所需依赖。
5. **验证**：
   ```bash
   cargo build                                    # 现有平台不回归
   cargo check --target aarch64-apple-darwin      # macOS 编译检查
   cargo check --target x86_64-pc-windows-msvc   # Windows 编译检查
   cargo test                                     # 所有 portable 测试通过
   ```

`widgex-core`、`widgex-ipc`（协议类型）、SolidJS 前端、TOML 配置格式均无需修改。
