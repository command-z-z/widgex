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
});
