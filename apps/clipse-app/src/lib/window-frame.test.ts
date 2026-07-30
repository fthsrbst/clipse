import { describe, expect, it } from "vitest";

import { RESIZE_EDGES, resizeDirection } from "./window-frame";

describe("resize edges", () => {
  it("covers all four sides and all four corners", () => {
    expect(RESIZE_EDGES).toHaveLength(8);
    expect(new Set(RESIZE_EDGES).size).toBe(8);
  });

  /** Tauri's `startResizeDragging` takes PascalCase direction names. A typo
   * here does not throw — the drag simply does nothing, on one edge only,
   * which is exactly the bug nobody finds by hand. */
  it("maps every edge to a Tauri direction name", () => {
    const expected = new Set([
      "North",
      "South",
      "East",
      "West",
      "NorthEast",
      "NorthWest",
      "SouthEast",
      "SouthWest",
    ]);
    const got = new Set(RESIZE_EDGES.map(resizeDirection));
    expect(got).toEqual(expected);
  });
});
