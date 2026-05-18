import { describe, expect, it } from "vitest";
import { normalizeWidgetTree } from "./widgetTree";

describe("normalizeWidgetTree", () => {
  it("keeps supported widgets and marks unknown widgets as errors", () => {
    const tree = normalizeWidgetTree([
      { type: "label", text: "Hello" },
      { type: "unknown-widget" },
    ]);

    expect(tree).toEqual([
      { type: "label", text: "Hello", children: [] },
      {
        type: "error",
        text: "Unsupported widget: unknown-widget",
        children: [],
      },
    ]);
  });

  it("preserves click, right-click, and scroll actions", () => {
    const action = { type: "command" as const, command: "true" };
    const tree = normalizeWidgetTree([
      {
        type: "box",
        on_click: action,
        on_right_click: action,
        on_scroll_up: action,
        on_scroll_down: action,
      },
    ]);

    expect(tree[0]).toMatchObject({
      on_click: action,
      on_right_click: action,
      on_scroll_up: action,
      on_scroll_down: action,
    });
  });
});
