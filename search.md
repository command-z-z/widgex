下面按“跟 Eww 相似程度”和“对你做新一代通用版的参考价值”来列。时间点按 2026-05-05。

  1. Eww 本体

  - 平台：Linux，X11/Wayland
  - 技术：Rust + GTK3 + gtk-layer-shell
  - 配置：yuck，样式 CSS/SCSS
  - 特点：窗口管理器无关，自定义 widget/bar 很灵活
  - 问题：只面向 Linux；yuck 对新手不算友好；GTK CSS 不是完整 Web CSS；动态复杂 UI 会绕

  参考：Eww 官方文档说它是 Rust 写的独立 widget system，用 yuck 配置、CSS 主题化。
  https://elkowar.github.io/eww/eww.html
  https://elkowar.github.io/eww/configuration.html

  2. AGS / Astal

  - 平台：Linux，偏 Wayland shell
  - 技术：GTK + GObject 生态，AGS 用 JavaScript/TypeScript/JSX，Astal 提供底层库
  - 配置：TS/JS/Lua 等代码式配置，样式 GTK CSS
  - 特点：比 Eww 更像“写一个桌面 shell 应用”；有电池、网络、蓝牙、音频、MPRIS、托盘等现成模块
  - 问题：门槛比 Eww 高，需要会编程；跨 Windows/macOS 基本不是目标

  适合参考它的 系统集成层和响应式状态绑定，不适合作为全平台方案原样照搬。
  https://aylur.github.io/ags/
  https://aylur.github.io/astal/

  3. Quickshell

  - 平台：Linux，Wayland/X11，Hyprland/Sway/i3 等集成强
  - 技术：QtQuick
  - 配置：QML
  - 特点：非常接近“新一代 Eww/AGS”，支持 bar、widgets、lockscreen、desktop shell；热重载；集成
    PipeWire、BlueZ、UPower、MPRIS、system tray 等
  - 问题：QML 对普通用户也算编程；官方也明确说它比 Waybar 这类状态栏更底层、更复杂；仍主要是 Linux 生态

  如果你目标是 Arch/Hyprland/Sway，这个是最值得重点研究的竞品。
  https://quickshell.org/
  https://quickshell.org/about/

  4. Fabric

  - 平台：Linux，X11/Wayland，Hyprland 支持
  - 技术：Python + GTK3/PyGObject
  - 配置：Python
  - 特点：用 Python 写 widget，类型提示和编辑器体验比较好；强调 signal-based workflow，减少 shell 脚本轮
    询
  - 问题：Python 运行时和依赖管理会变复杂；跨 macOS/Windows 桌面 shell 不是主要目标；长期性能和打包体验
    不如 Rust 原生路线

  适合参考它的 开发者体验、内置 widget、Python 生态接入。
  https://wiki.ffpy.org/getting-started/introduction/
  https://github.com/Fabric-Development/fabric

  5. Waybar

  - 平台：Linux Wayland，Sway/wlroots/Hyprland 等
  - 技术：C++/GTK
  - 配置：JSON/JSONC + CSS
  - 特点：状态栏领域事实标准之一，轻量、稳定、模块丰富
  - 问题：它主要是 bar，不是通用 widget system；布局和交互自由度不如 Eww/AGS/Quickshell

  适合参考它的 配置简单性和模块边界。
  官方 GitHub 也提醒 Waybar 没有官方独立网站。
  https://github.com/Alexays/Waybar

  6. nwg-panel

  - 平台：Linux Wayland，Sway/Hyprland
  - 技术：GTK3
  - 配置：带图形配置工具
  - 特点：比 Eww 更“产品化”，用户不用全手写配置；模块固定，适合作为 panel
  - 问题：扩展性明显弱于 Eww/AGS/Quickshell；不是通用 widget runtime

  适合参考它的 GUI 配置器。
  https://nwg-piotr.github.io/nwg-shell/nwg-panel.html
  https://github.com/nwg-piotr/nwg-panel

  7. Conky

  - 平台：Linux/BSD 等，部分 macOS
  - 技术：C++，可扩展 Lua
  - 配置：Conky config + Lua
  - 特点：老牌系统监控桌面 widget，极轻，系统信息能力强
  - 问题：现代交互 UI、Wayland shell、复杂组件体验不强；更像系统监控 overlay

  适合参考它的 低资源占用和系统指标采集。
  https://github.com/brndnmtthws/conky
  https://portable-linux-apps.github.io/apps/conky.html

  8. Rainmeter

  - 平台：Windows
  - 技术：C++，插件可用 C#/其他
  - 配置：.ini skins
  - 特点：Windows 桌面美化和 widget 生态最成熟，社区 skin 很多
  - 问题：Windows-only；配置体系历史包袱重；跨平台不可直接复用

  如果你要做 Windows 版 Eww，Rainmeter 是必须研究的对象。
  https://docs.rainmeter.net/manual/getting-started/
  https://github.com/rainmeter/rainmeter

  9. Übersicht

  - 平台：macOS
  - 技术：HTML5 + JavaScript + React JSX
  - 配置：JS/JSX widget
  - 特点：在 macOS 桌面显示系统命令输出，写 widget 很像写前端
  - 问题：macOS-only；安全模型较弱，因为 widget 可以跑系统命令；系统级 bar/panel 能力有限

  适合参考它的 前端式 widget 开发体验。
  https://tracesof.net/uebersicht/

  10. xbar / BitBar

  - 平台：macOS 菜单栏
  - 技术：Go 应用，插件是任意脚本
  - 配置：脚本 stdout 输出
  - 特点：极简单，任何语言都能写插件；适合菜单栏信息和操作入口
  - 问题：不是桌面 widget，也不是复杂 UI runtime

  适合参考它的 插件协议极简设计。
  https://xbarapp.com/
  https://github.com/matryer/xbar

  总体对比

  Eww        = Linux 通用 widget，声明式 DSL，灵活但跨平台弱
  AGS/Astal  = Linux shell 编程框架，系统集成强，门槛高
  Quickshell = Linux 新派 desktop shell，QML/QtQuick，最像强竞品
  Fabric     = Python 版 Linux widget framework，开发体验好
  Waybar     = Wayland 状态栏标准，稳定但范围窄
  nwg-panel  = 带 GUI 配置的 Wayland panel，易用但扩展弱
  Conky      = 系统监控 overlay 老牌工具，轻但现代 UI 弱
  Rainmeter  = Windows 桌面 widget 生态标杆，平台单一
  Übersicht  = macOS HTML/JS 桌面 widget，前端友好
  xbar       = macOS 菜单栏脚本插件，协议极简

  我的判断：如果你要做“全平台新一代 Eww”，最该重点研究三类：

  Linux 竞品：Quickshell / AGS / Fabric
  Windows 标杆：Rainmeter
  macOS 标杆：Übersicht / xbar

  技术方向上，Rust + Slint 更像是在做一个“轻量、原生、跨平台 Rainmeter/Eww 内核”；Tauri + Web 更像是在
  做“跨平台 Übersicht + Eww”。如果你优先照顾普通用户配置体验，我会让配置层学习 Waybar/nwg-panel：声明
  式、schema 补全、可视化配置器，而不是让用户一上来写 QML/TS/Python。
