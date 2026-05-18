import { createEffect, createSignal, For, onCleanup } from "solid-js";
import { createStore, reconcile } from "solid-js/store";
import type {
  NormalizedWidgetNode,
  RawWidgetNode,
  WidgetAction,
} from "./widgetTree";
import { normalizeWidgetTree } from "./widgetTree";

/** Mirrors the Rust `RendererPayload`. Widget `text`/`value` arrive already
 *  resolved — bindings are evaluated daemon-side (single source of truth). */
type RendererWindow = {
  id: string;
  title?: string | null;
  widgets: RawWidgetNode[];
};

type RendererPayload = {
  version: number;
  theme_css: string | null;
  theme_css_files?: string[];
  windows: RendererWindow[];
};

declare global {
  interface Window {
    /** Initial payload, injected by the host before page scripts run. */
    __WIDGEX_PAYLOAD__?: RendererPayload;
    /** Live update hook the host calls on every poll tick. */
    __widgexPush?: (payload: RendererPayload) => void;
    /** Payloads buffered before __widgexPush is initialized; drained on mount. */
    __widgex_queue?: RendererPayload[] | null;
    /** Window ID injected by the multi-window renderer so each webview knows
     *  which window in the payload it should render. */
    __WIDGEX_WINDOW_ID__?: string;
    ipc?: {
      postMessage: (message: string) => void;
    };
  }
}

const EMPTY_PAYLOAD: RendererPayload = {
  version: 1,
  theme_css: null,
  windows: [],
};

function pickWindow(payload: RendererPayload): RendererWindow | undefined {
  const id = window.__WIDGEX_WINDOW_ID__;
  if (id) return payload.windows.find((w) => w.id === id) ?? payload.windows[0];
  return payload.windows[0];
}

export function App() {
  const initial = window.__WIDGEX_PAYLOAD__ ?? EMPTY_PAYLOAD;

  // createStore + reconcile: each push patches widget objects in-place on the
  // same Proxy references. <For> sees stable identities → never unmounts nodes
  // → button click handlers are always live, no dropped clicks.
  const [widgets, setWidgets] = createStore(
    normalizeWidgetTree(pickWindow(initial)?.widgets ?? []),
  );
  const [themeCss, setThemeCss] = createSignal(initial.theme_css ?? "");

  window.__widgexPush = (next) => {
    setWidgets(reconcile(normalizeWidgetTree(pickWindow(next)?.widgets ?? [])));
    setThemeCss(next.theme_css ?? "");
  };

  // Drain payloads that were queued before this function was defined.
  const pending = window.__widgex_queue;
  window.__widgex_queue = null;
  if (pending && pending.length > 0) {
    window.__widgexPush(pending[pending.length - 1]);
  }

  const styleElement = document.createElement("style");
  styleElement.dataset.widgexTheme = "true";
  document.head.appendChild(styleElement);
  createEffect(() => {
    styleElement.textContent = themeCss();
  });
  onCleanup(() => styleElement.remove());

  return (
    <main class="widgex-window">
      <WidgetList widgets={widgets} />
    </main>
  );
}

function WidgetList(props: { widgets: NormalizedWidgetNode[] }) {
  return (
    <For each={props.widgets}>
      {(widget) => <WidgetNode widget={widget} />}
    </For>
  );
}

function classList(base: string, extra?: string[]): string {
  return extra && extra.length > 0 ? `${base} ${extra.join(" ")}` : base;
}

function sendAction(action?: WidgetAction, value?: string) {
  if (!action) return;
  window.ipc?.postMessage(JSON.stringify({ action, value }));
}

function stopAndSendAction(event: MouseEvent, action?: WidgetAction) {
  if (!action) return;
  event.preventDefault();
  event.stopPropagation();
  sendAction(action);
}

function stopAndSendWheelAction(
  event: WheelEvent,
  scrollUp?: WidgetAction,
  scrollDown?: WidgetAction,
) {
  const action = event.deltaY < 0 ? scrollUp : event.deltaY > 0 ? scrollDown : undefined;
  if (!action) return;
  event.preventDefault();
  event.stopPropagation();
  sendAction(action);
}

function AnimationWidget(props: { widget: NormalizedWidgetNode }) {
  const fw = () => props.widget.frame_width ?? 192;
  const fh = () => props.widget.frame_height ?? 208;
  const cols = () => props.widget.cols ?? 1;
  const row = () => parseInt(props.widget.frame_row ?? "0", 10) || 0;
  const durations = () => props.widget.frame_durations ?? [150];
  const numFrames = () =>
    parseInt(props.widget.frame_count ?? "", 10) || durations().length;

  // When draw_x/draw_y are set: canvas fills parent (full-screen), sprite drawn at
  // (draw_x, draw_y) within the canvas. The canvas element never moves — only pixel
  // content changes — so webkit2gtk incremental repaints leave no ghost artifacts.
  const hasPosition = () =>
    props.widget.draw_x !== undefined && props.widget.draw_y !== undefined;
  const drawX = () => parseInt(props.widget.draw_x ?? "0", 10) || 0;
  const drawY = () => parseInt(props.widget.draw_y ?? "0", 10) || 0;

  const [frame, setFrame] = createSignal(0);
  const [img, setImg] = createSignal<HTMLImageElement | null>(null);
  let canvasRef: HTMLCanvasElement | undefined;

  // Load spritesheet reactively; re-runs if src changes
  createEffect(() => {
    const src = props.widget.src ?? "";
    if (!src) { setImg(null); return; }
    const el = new Image();
    el.onload = () => setImg(el);
    el.onerror = () => setImg(null);
    el.src = src;
  });

  // Animation tick loop
  createEffect(() => {
    const nf = numFrames();
    const timings = durations();
    let idx = 0;
    let tid: ReturnType<typeof setTimeout>;
    const tick = () => {
      setFrame(idx);
      tid = setTimeout(tick, timings[idx] ?? 150);
      idx = (idx + 1) % nf;
    };
    tick();
    onCleanup(() => clearTimeout(tid));
  });

  // Paint effect.
  // webkit2gtk transparent-window DMA-BUF ghost workaround:
  // clearRect() to fully-transparent may be treated as "undamaged" by webkit,
  // leaving old pixels in Hyprland's previous buffer. fillRect with near-zero
  // alpha forces webkit to mark the whole canvas as actively drawn content,
  // which propagates correct damage and clears the ghost.
  createEffect(() => {
    const canvas = canvasRef;
    if (!canvas) return;
    const image = img();
    const f = frame();
    const r = row();
    const frameW = fw();
    const frameH = fh();
    void cols();
    const ctx = canvas.getContext("2d");
    if (!ctx) return;
    ctx.clearRect(0, 0, canvas.width, canvas.height);
    if (!image) return;
    ctx.drawImage(image, f * frameW, r * frameH, frameW, frameH,
                  drawX(), drawY(), frameW, frameH);
  });

  const setupCanvas = (el: HTMLCanvasElement) => {
    canvasRef = el;
    if (hasPosition()) {
      const setSize = () => {
        el.width = el.offsetWidth || window.innerWidth;
        el.height = el.offsetHeight || window.innerHeight;
      };
      setSize();
      window.addEventListener("resize", setSize);
      onCleanup(() => window.removeEventListener("resize", setSize));
    } else {
      el.width = fw();
      el.height = fh();
    }
  };

  return (
    <canvas
      ref={setupCanvas}
      class={props.widget.class?.join(" ")}
      style={hasPosition()
        ? { width: "100%", height: "100%", display: "block" }
        : { width: `${fw()}px`, height: `${fh()}px`, display: "block" }}
    />
  );
}

function progressStyle(widget: NormalizedWidgetNode): string {
  const value = Math.max(0, Math.min(100, Number(widget.value) || 0));
  const base = widget.style ? `${widget.style}; ` : "";
  return `${base}--widgex-progress-value: ${value}`;
}

function WidgetNode(props: { widget: NormalizedWidgetNode }) {
  const widget = props.widget;

  switch (widget.type) {
    case "box":
      return (
        <div
          class={classList(
            `widgex-box widgex-box-${widget.direction ?? "row"}`,
            widget.class,
          )}
          style={widget.style}
          onClick={(event) => stopAndSendAction(event, widget.on_click)}
          onContextMenu={(event) => stopAndSendAction(event, widget.on_right_click)}
          onWheel={(event) =>
            stopAndSendWheelAction(event, widget.on_scroll_up, widget.on_scroll_down)
          }
        >
          <WidgetList widgets={widget.children} />
        </div>
      );
    case "label":
      return (
        <span class={classList("widgex-label", widget.class)} style={widget.style}>
          {widget.text}
        </span>
      );
    case "button":
      return (
        <button
          class={classList("widgex-button", widget.class)}
          style={widget.style}
          onClick={(event) => stopAndSendAction(event, widget.on_click)}
          onContextMenu={(event) => stopAndSendAction(event, widget.on_right_click)}
          onWheel={(event) =>
            stopAndSendWheelAction(event, widget.on_scroll_up, widget.on_scroll_down)
          }
        >
          {widget.text}
        </button>
      );
    case "image":
      return (
        <img
          class={classList("widgex-image", widget.class)}
          src={widget.src}
          style={widget.style}
        />
      );
    case "progress":
      return (
        <input
          type="range"
          class={classList("widgex-progress", widget.class)}
          min="0"
          max="100"
          value={Number(widget.value) || 0}
          style={progressStyle(widget)}
          onChange={(event) => sendAction(widget.on_change, event.currentTarget.value)}
        />
      );
    case "spacer":
      return <div class={classList("widgex-spacer", widget.class)} style={widget.style} />;
    case "animation":
      return <AnimationWidget widget={widget} />;
    case "error":
      return <span class="widgex-error">{widget.text}</span>;
    default:
      return <span class="widgex-placeholder">{widget.type}</span>;
  }
}
