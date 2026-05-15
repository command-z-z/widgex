import { createSignal, For, Show } from "solid-js";
import type { NormalizedWidgetNode, RawWidgetNode } from "./widgetTree";
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
  windows: RendererWindow[];
};

declare global {
  interface Window {
    /** Initial payload, injected by the host before page scripts run. */
    __WIDGEX_PAYLOAD__?: RendererPayload;
    /** Live update hook the host calls on every poll tick. */
    __widgexPush?: (payload: RendererPayload) => void;
  }
}

const EMPTY_PAYLOAD: RendererPayload = {
  version: 1,
  theme_css: null,
  windows: [],
};

export function App() {
  const [payload, setPayload] = createSignal<RendererPayload>(
    window.__WIDGEX_PAYLOAD__ ?? EMPTY_PAYLOAD,
  );

  // Register before the first poll tick so no update is dropped.
  window.__widgexPush = (next) => setPayload(next);

  const widgets = () =>
    normalizeWidgetTree(payload().windows[0]?.widgets ?? []);
  const themeCss = () => payload().theme_css ?? "";

  return (
    <main class="widgex-window">
      <Show when={themeCss()}>
        <style>{themeCss()}</style>
      </Show>
      <WidgetList widgets={widgets()} />
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
        >
          <WidgetList widgets={widget.children} />
        </div>
      );
    case "label":
      return (
        <span class={classList("widgex-label", widget.class)}>
          {widget.text}
        </span>
      );
    case "button":
      return (
        <button class={classList("widgex-button", widget.class)}>
          {widget.text}
        </button>
      );
    case "image":
      return (
        <img class={classList("widgex-image", widget.class)} src={widget.src} />
      );
    case "progress":
      return (
        <progress
          class={classList("widgex-progress", widget.class)}
          max="1"
          value={Number(widget.value) || 0}
        />
      );
    case "spacer":
      return <div class={classList("widgex-spacer", widget.class)} />;
    case "error":
      return <span class="widgex-error">{widget.text}</span>;
    default:
      return <span class="widgex-placeholder">{widget.type}</span>;
  }
}
