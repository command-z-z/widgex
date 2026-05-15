export type RawWidgetNode = {
  type?: string;
  id?: string;
  class?: string[];
  text?: string;
  value?: string;
  src?: string;
  direction?: "row" | "column";
  children?: RawWidgetNode[];
};

export type NormalizedWidgetNode = {
  type: string;
  id?: string;
  class?: string[];
  text?: string;
  value?: string;
  src?: string;
  direction?: "row" | "column";
  children: NormalizedWidgetNode[];
};

const SUPPORTED_WIDGETS = new Set([
  "box",
  "label",
  "button",
  "image",
  "progress",
  "spacer",
]);

export function normalizeWidgetTree(
  widgets: RawWidgetNode[],
): NormalizedWidgetNode[] {
  return widgets.map(normalizeWidget);
}

function normalizeWidget(widget: RawWidgetNode): NormalizedWidgetNode {
  if (!widget.type || !SUPPORTED_WIDGETS.has(widget.type)) {
    return {
      type: "error",
      text: `Unsupported widget: ${widget.type ?? "missing type"}`,
      children: [],
    };
  }

  return {
    type: widget.type,
    id: widget.id,
    class: widget.class,
    text: widget.text,
    value: widget.value,
    src: widget.src,
    direction: widget.direction,
    children: normalizeWidgetTree(widget.children ?? []),
  };
}
