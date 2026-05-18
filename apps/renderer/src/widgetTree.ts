export type RawWidgetNode = {
  type?: string;
  id?: string;
  class?: string[];
  text?: string;
  value?: string;
  src?: string;
  frame_width?: number;
  frame_height?: number;
  cols?: number;
  frame_row?: string;
  frame_count?: string;
  draw_x?: string;
  draw_y?: string;
  frame_durations?: number[];
  style?: string;
  direction?: "row" | "column";
  on_click?: WidgetAction;
  on_change?: WidgetAction;
  on_right_click?: WidgetAction;
  on_scroll_up?: WidgetAction;
  on_scroll_down?: WidgetAction;
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
  frame_width?: number;
  frame_height?: number;
  cols?: number;
  frame_row?: string;
  frame_count?: string;
  draw_x?: string;
  draw_y?: string;
  frame_durations?: number[];
  style?: string;
  direction?: "row" | "column";
  on_click?: WidgetAction;
  on_change?: WidgetAction;
  on_right_click?: WidgetAction;
  on_scroll_up?: WidgetAction;
  on_scroll_down?: WidgetAction;
  children: NormalizedWidgetNode[];
};

const SUPPORTED_WIDGETS = new Set([
  "box",
  "label",
  "button",
  "image",
  "progress",
  "spacer",
  "animation",
  "canvas",
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
  if (widget.frame_width !== undefined) normalized.frame_width = widget.frame_width;
  if (widget.frame_height !== undefined) normalized.frame_height = widget.frame_height;
  if (widget.cols !== undefined) normalized.cols = widget.cols;
  if (widget.frame_row !== undefined) normalized.frame_row = widget.frame_row;
  if (widget.frame_count !== undefined) normalized.frame_count = widget.frame_count;
  if (widget.draw_x !== undefined) normalized.draw_x = widget.draw_x;
  if (widget.draw_y !== undefined) normalized.draw_y = widget.draw_y;
  if (widget.frame_durations !== undefined) normalized.frame_durations = widget.frame_durations;
  if (widget.style !== undefined) normalized.style = widget.style;
  if (widget.direction !== undefined) normalized.direction = widget.direction;
  if (widget.on_click !== undefined) normalized.on_click = widget.on_click;
  if (widget.on_change !== undefined) normalized.on_change = widget.on_change;
  if (widget.on_right_click !== undefined) normalized.on_right_click = widget.on_right_click;
  if (widget.on_scroll_up !== undefined) normalized.on_scroll_up = widget.on_scroll_up;
  if (widget.on_scroll_down !== undefined) normalized.on_scroll_down = widget.on_scroll_down;
  return normalized;
}
