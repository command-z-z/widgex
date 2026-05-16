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

export function App() {
  const initial = window.__WIDGEX_PAYLOAD__ ?? EMPTY_PAYLOAD;

  // createStore + reconcile: each push patches widget objects in-place on the
  // same Proxy references. <For> sees stable identities → never unmounts nodes
  // → button click handlers are always live, no dropped clicks.
  const [widgets, setWidgets] = createStore(
    normalizeWidgetTree(initial.windows[0]?.widgets ?? []),
  );
  const [themeCss, setThemeCss] = createSignal(initial.theme_css ?? "");

  window.__widgexPush = (next) => {
    setWidgets(reconcile(normalizeWidgetTree(next.windows[0]?.widgets ?? [])));
    setThemeCss(next.theme_css ?? "");
  };

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
          onClick={() => sendAction(widget.on_click)}
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
    case "error":
      return <span class="widgex-error">{widget.text}</span>;
    default:
      return <span class="widgex-placeholder">{widget.type}</span>;
  }
}
