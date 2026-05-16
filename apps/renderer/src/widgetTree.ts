export type RawWidgetNode = {
  type?: string;
  id?: string;
  class?: string[];
  text?: string;
  value?: string;
  src?: string;
  style?: string;
  direction?: "row" | "column";
  on_click?: WidgetAction;
  on_change?: WidgetAction;
  children?: RawWidgetNode[];
};

export type WidgetAction = {
  type: "command" | "emit";
  command?: string;
  event?: string;
};

export type NormalizedWidgetNode = {
  type: string;
  id?: string;
  class?: string[];
  text?: string;
  value?: string;
  src?: string;
  style?: string;
  direction?: "row" | "column";
  on_click?: WidgetAction;
  on_change?: WidgetAction;
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

  const normalized: NormalizedWidgetNode = {
    type: widget.type,
    children: normalizeWidgetTree(widget.children ?? []),
  };
  if (widget.id !== undefined) normalized.id = widget.id;
  if (widget.class !== undefined) normalized.class = widget.class;
  if (widget.text !== undefined) normalized.text = widget.text;
  if (widget.value !== undefined) normalized.value = widget.value;
  if (widget.src !== undefined) normalized.src = widget.src;
  if (widget.style !== undefined) normalized.style = widget.style;
  if (widget.direction !== undefined) normalized.direction = widget.direction;
  if (widget.on_click !== undefined) normalized.on_click = widget.on_click;
  if (widget.on_change !== undefined) normalized.on_change = widget.on_change;
  return normalized;
}
