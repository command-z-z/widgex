import { createEffect, createSignal, For, onCleanup, onMount } from "solid-js";
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

// --- Canvas particle engine ---

interface ParticleParams {
  mode: "snow" | "leaves" | "stars";
  count: number;
  speed: number;
  alpha: number;
  wind: number;
}

function tryParseParams(value?: string): ParticleParams {
  const defaults: ParticleParams = { mode: "snow", count: 120, speed: 1.0, alpha: 0.85, wind: 0.2 };
  if (!value) return defaults;
  try {
    return { ...defaults, ...JSON.parse(value) };
  } catch {
    return defaults;
  }
}

interface Particle {
  x: number; y: number; r: number; vx: number; vy: number;
  alpha: number; alphaDelta: number;
  // leaves
  rotation?: number; rotationSpeed?: number; w?: number; h?: number; color?: string;
  // stars: shooting star fields
  isShooting?: boolean; length?: number; age?: number; maxAge?: number;
}

const LEAF_COLORS = ["#c45c13", "#d4750a", "#b8860b", "#8b6914", "#c47a1d", "#9b3a00"];

function createParticleEngine(canvas: HTMLCanvasElement, params: ParticleParams) {
  const { mode, count, speed, alpha: baseAlpha, wind } = params;
  const W = () => canvas.width;
  const H = () => canvas.height;

  const particles: Particle[] = [];

  function spawnSnow(p: Particle) {
    p.x = Math.random() * W();
    p.y = Math.random() * H();
    p.r = 2 + Math.random() * 4;
    p.vx = (Math.random() - 0.5) * wind * 2;
    p.vy = (0.4 + Math.random() * 0.8) * speed;
    p.alpha = 0.5 + Math.random() * 0.5;
    p.alphaDelta = 0;
  }

  function spawnLeaf(p: Particle) {
    p.x = Math.random() * W();
    p.y = -20 + Math.random() * 40 - 40;
    p.w = 8 + Math.random() * 8;
    p.h = 4 + Math.random() * 6;
    p.vx = (Math.random() - 0.5) * wind * 4 + (Math.random() > 0.5 ? 0.3 : -0.3);
    p.vy = (0.5 + Math.random() * 1.2) * speed;
    p.rotation = Math.random() * Math.PI * 2;
    p.rotationSpeed = (Math.random() - 0.5) * 0.05;
    p.color = LEAF_COLORS[Math.floor(Math.random() * LEAF_COLORS.length)];
    p.alpha = 0.6 + Math.random() * 0.4;
    p.alphaDelta = 0;
    p.r = 0;
  }

  function spawnStar(p: Particle) {
    p.x = Math.random() * W();
    p.y = Math.random() * H();
    p.r = Math.random() < 0.15 ? 2 : 1;
    p.vx = 0; p.vy = 0;
    p.alpha = 0.3 + Math.random() * 0.7;
    p.alphaDelta = (Math.random() - 0.5) * 0.008;
    p.isShooting = false;
  }

  function spawnShooting(p: Particle) {
    p.x = Math.random() * W();
    p.y = Math.random() * (H() * 0.5);
    p.vx = 4 + Math.random() * 6;
    p.vy = 2 + Math.random() * 4;
    p.length = 40 + Math.random() * 80;
    p.alpha = 1;
    p.alphaDelta = -0.03;
    p.age = 0;
    p.maxAge = 30 + Math.floor(Math.random() * 20);
    p.isShooting = true;
    p.r = 0;
  }

  const spawn = mode === "snow" ? spawnSnow : mode === "leaves" ? spawnLeaf : spawnStar;
  for (let i = 0; i < count; i++) {
    const p: Particle = { x: 0, y: 0, r: 0, vx: 0, vy: 0, alpha: 0, alphaDelta: 0 };
    spawn(p);
    if (mode === "snow") p.y = Math.random() * H(); // scatter vertically at start
    particles.push(p);
  }

  // A few shooting stars pre-allocated
  const shooters: Particle[] = [];
  if (mode === "stars") {
    for (let i = 0; i < 3; i++) {
      const p: Particle = { x: 0, y: 0, r: 0, vx: 0, vy: 0, alpha: 0, alphaDelta: 0 };
      spawnShooting(p);
      p.age = p.maxAge; // start as "done" so they don't all fire at once
      shooters.push(p);
    }
  }

  return {
    tick() {
      const ctx = canvas.getContext("2d");
      if (!ctx) return;
      ctx.clearRect(0, 0, W(), H());

      if (mode === "snow") {
        for (const p of particles) {
          p.x += p.vx + Math.sin(Date.now() / 2000 + p.y * 0.02) * 0.3 * wind;
          p.y += p.vy;
          if (p.y > H() + p.r) { spawnSnow(p); p.y = -p.r; }
          if (p.x < -p.r) p.x = W() + p.r;
          if (p.x > W() + p.r) p.x = -p.r;
          ctx.beginPath();
          ctx.arc(p.x, p.y, p.r, 0, Math.PI * 2);
          ctx.fillStyle = `rgba(255,255,255,${(p.alpha * baseAlpha).toFixed(2)})`;
          ctx.fill();
        }
      } else if (mode === "leaves") {
        for (const p of particles) {
          p.x += p.vx + Math.sin(Date.now() / 1500 + p.y * 0.01) * wind;
          p.y += p.vy;
          p.rotation! += p.rotationSpeed!;
          if (p.y > H() + 20) spawnLeaf(p);
          ctx.save();
          ctx.translate(p.x, p.y);
          ctx.rotate(p.rotation!);
          ctx.globalAlpha = p.alpha * baseAlpha;
          ctx.fillStyle = p.color!;
          ctx.beginPath();
          ctx.ellipse(0, 0, p.w! / 2, p.h! / 2, 0, 0, Math.PI * 2);
          ctx.fill();
          ctx.restore();
          ctx.globalAlpha = 1;
        }
      } else {
        // stars
        for (const p of particles) {
          p.alpha += p.alphaDelta;
          if (p.alpha > 1) { p.alpha = 1; p.alphaDelta = -Math.abs(p.alphaDelta); }
          if (p.alpha < 0.1) { p.alpha = 0.1; p.alphaDelta = Math.abs(p.alphaDelta); }
          ctx.beginPath();
          ctx.arc(p.x, p.y, p.r, 0, Math.PI * 2);
          ctx.fillStyle = `rgba(255,255,255,${(p.alpha * baseAlpha).toFixed(2)})`;
          ctx.fill();
        }
        // shooting stars
        for (const p of shooters) {
          p.age = (p.age ?? 0) + 1;
          if (p.age >= p.maxAge!) {
            // random re-trigger: ~2% chance per frame once expired
            if (Math.random() < 0.005) spawnShooting(p);
            continue;
          }
          p.x += p.vx!;
          p.y += p.vy!;
          p.alpha += p.alphaDelta;
          if (p.alpha < 0) p.alpha = 0;
          const dx = -(p.vx! / Math.hypot(p.vx!, p.vy!)) * p.length!;
          const dy = -(p.vy! / Math.hypot(p.vx!, p.vy!)) * p.length!;
          const grad = ctx.createLinearGradient(p.x, p.y, p.x + dx, p.y + dy);
          grad.addColorStop(0, `rgba(255,255,255,${p.alpha.toFixed(2)})`);
          grad.addColorStop(1, "rgba(255,255,255,0)");
          ctx.beginPath();
          ctx.moveTo(p.x, p.y);
          ctx.lineTo(p.x + dx, p.y + dy);
          ctx.strokeStyle = grad;
          ctx.lineWidth = 1.5;
          ctx.stroke();
        }
      }
    },
  };
}

function CanvasWidget(props: { widget: NormalizedWidgetNode }) {
  let canvasRef!: HTMLCanvasElement;

  onMount(() => {
    const params = tryParseParams(props.widget.value);

    const resize = () => {
      canvasRef.width = canvasRef.offsetWidth || window.innerWidth;
      canvasRef.height = canvasRef.offsetHeight || window.innerHeight;
    };
    resize();
    const observer = new ResizeObserver(resize);
    observer.observe(canvasRef);

    const engine = createParticleEngine(canvasRef, params);
    let rafId = requestAnimationFrame(function loop() {
      engine.tick();
      rafId = requestAnimationFrame(loop);
    });

    onCleanup(() => {
      cancelAnimationFrame(rafId);
      observer.disconnect();
    });
  });

  return (
    <canvas
      ref={canvasRef!}
      class={props.widget.class?.join(" ")}
      style={{ width: "100%", height: "100%", display: "block" }}
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
    case "canvas":
      return <CanvasWidget widget={widget} />;
    case "error":
      return <span class="widgex-error">{widget.text}</span>;
    default:
      return <span class="widgex-placeholder">{widget.type}</span>;
  }
}
