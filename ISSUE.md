# Known Issues

## Ghosting on transparent layer-shell windows (webkit2gtk damage residue)

**Status**: Unresolved (upstream bug, no official fix)

**Impact**: On transparent windows (layer-shell surfaces created with
`with_transparent(true)`), old-frame pixels are not cleared after content
updates or animated transforms, leaving stacked ghost images on screen.
Typical symptoms: `lyrics_overlay` shows stale lyrics overlapping the new
line on line changes; entrance animations using `transform` translation
leave a residual strip on low-refresh windows.

**Root cause**: A known bug in `wry` + `webkit2gtk` + Hyprland
([wry #1524](https://github.com/tauri-apps/wry/issues/1524) — still OPEN,
no official fix). On a transparent layer-shell surface webkit performs
incremental repaints (damage propagation), repainting only the damaged
region and leaving the previous frame's buffer pixels behind. The issue
reporter confirmed it reproduces on Hyprland but not on Sway.

**Environment**: Arch Linux / Hyprland / webkit2gtk-4.1 2.52 / wry 0.55

**Ruled-out approaches**:
- `WEBKIT_DISABLE_COMPOSITING_MODE=1` has no effect — webkit 2.52 still does
  damage propagation in non-compositing mode.
- The only workaround in wry #1524 (resize the window to 1x1 and back) could
  not be reproduced even by the issue reporter.
- The `webkit2gtk` crate 2.0.2 does not bind the feature-flag API needed to
  disable the damage feature (requires v2_42+); `wry` 0.55 does not expose the
  underlying `WebKitWebView` either.

**Current mitigation**: Give the ghosting region an opaque background so every
frame paints over old pixels with solid color.
- `lyrics_overlay`: container uses an opaque background.
- Desktop widgets (clock/date/memo/weather): cards use opaque backgrounds;
  entrance animations avoid `transform` translation (opacity-only fade), since
  any positional animation on a transparent window triggers the ghosting.

**References**:
- https://github.com/tauri-apps/wry/issues/1524
- https://github.com/DioxusLabs/dioxus/issues/3821

---

## wl_shm buffer accumulation on transparent layer-shell windows (memory growth)

**Status**: Unresolved (upstream bug — filed upstream, no fix yet)

**Impact**: `WebKitWebProcess` RSS grows unboundedly on transparent `zwlr_layer_shell_v1` windows. Growth rate is proportional to repaint frequency. At ~1–3 repaints/second (clock + memo + sysstat), accumulation is ~1.37 buffers/second (~100 KB/s). Over hours this reaches hundreds of MB.

**Root cause**: Hyprland does not send `wl_buffer.release` events to clients for transparent layer-shell surfaces. Per the Wayland protocol, the compositor must release a committed buffer once it no longer reads from it — without this signal, webkit2gtk cannot reuse or free old wl_shm allocations and allocates a fresh `/memfd:WebKitSharedMemory` buffer on every repaint. This is likely a side-effect of Hyprland's damage-only repaint strategy on transparent surfaces (same root as the ghosting bug above): old frame buffers are retained as the compositing base for new damage regions, and `wl_buffer.release` is never sent for them.

**Observed data (15 min runtime)**:
- `/memfd:WebKitSharedMemory` buffer count: 2034 (expected: 2–4 per window)
- Total mapped size: ~153 MB (1504 × 72 KB + 530 × 84 KB)
- All buffers mapped `r--s` — held live by compositor

**Environment**: Arch Linux / Hyprland 0.55.2 / webkit2gtk-4.1 2.52.3 / wry 0.55.1

**Mitigations applied**:
- Per-window payload filtering in `run_renderer` tick loop (`crates/widgex-webview/src/lib.rs`): `evaluate_script` is only called when that window's own data changed, reducing unnecessary repaints (fixed runaway 7 MB/s growth; residual accumulation is now ~100 KB/s).
- Merged memo widget's 10 independent poll sources into one (`~/.config/widgex/widgets/memo/config.toml`): reduces repaint triggers from 10 × 0.67/s to 1 × 0.67/s for that window.
- `native_render = true` on the `pet` window: GTK widget tree renderer bypasses WebKit entirely, zero accumulation for that window.

**Ruled-out approaches**:
- `WEBKIT_DISABLE_DMABUF_RENDERER=1`: no effect on buffer accumulation
- Reducing `evaluate_script` payload size: does not affect whether `wl_buffer.release` is sent

**Fundamental workaround**: Use `native_render = true` (GTK widget tree) for any window that requires transparency. WebKit-backed transparent windows will always accumulate at a rate equal to their repaint frequency until Hyprland or webkit2gtk fixes the release behavior.

**References**:
- https://github.com/tauri-apps/wry/issues/1731 (filed)
- https://github.com/hyprwm/Hyprland/discussions/14663 (filed)
