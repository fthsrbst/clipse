import { describe, expect, it } from "vitest";
import { computeVirtualRange } from "./virtual-list";

describe("computeVirtualRange", () => {
  it("renders nothing for an empty list", () => {
    const range = computeVirtualRange({ itemCount: 0, itemHeight: 40, containerHeight: 400, scrollTop: 0 });
    expect(range).toEqual({ startIndex: 0, endIndex: 0, offsetTop: 0, totalHeight: 0 });
  });

  it("computes total height from item count and height alone", () => {
    const range = computeVirtualRange({ itemCount: 50_000, itemHeight: 40, containerHeight: 400, scrollTop: 0 });
    expect(range.totalHeight).toBe(50_000 * 40);
  });

  it("never renders anywhere close to the full unbounded list", () => {
    const range = computeVirtualRange({ itemCount: 50_000, itemHeight: 40, containerHeight: 400, scrollTop: 20_000 * 40 });
    expect(range.endIndex - range.startIndex).toBeLessThan(50);
  });

  it("starts at the top with no overscan underflow", () => {
    const range = computeVirtualRange({ itemCount: 1000, itemHeight: 40, containerHeight: 400, scrollTop: 0, overscan: 4 });
    expect(range.startIndex).toBe(0);
    expect(range.offsetTop).toBe(0);
  });

  it("applies overscan above and below the visible window", () => {
    // 400 / 40 = 10 visible rows; scrolled to row 100.
    const range = computeVirtualRange({
      itemCount: 1000,
      itemHeight: 40,
      containerHeight: 400,
      scrollTop: 100 * 40,
      overscan: 4,
    });
    expect(range.startIndex).toBe(96); // 100 - 4
    expect(range.endIndex).toBe(114); // 100 + 10 + 4
    expect(range.offsetTop).toBe(96 * 40);
  });

  it("clamps the end index to the item count near the bottom", () => {
    const range = computeVirtualRange({
      itemCount: 105,
      itemHeight: 40,
      containerHeight: 400,
      scrollTop: 100 * 40,
      overscan: 4,
    });
    expect(range.endIndex).toBe(105);
    expect(range.endIndex).toBeLessThanOrEqual(105);
  });

  it("clamps a negative scrollTop to zero instead of underflowing", () => {
    const range = computeVirtualRange({ itemCount: 100, itemHeight: 40, containerHeight: 400, scrollTop: -500 });
    expect(range.startIndex).toBe(0);
  });

  it("always renders at least one row worth of visible space, even in a tiny container", () => {
    const range = computeVirtualRange({ itemCount: 100, itemHeight: 40, containerHeight: 5, scrollTop: 0 });
    expect(range.endIndex).toBeGreaterThan(range.startIndex);
  });

  it("keeps the window size stable as the user scrolls through the middle", () => {
    const a = computeVirtualRange({ itemCount: 50_000, itemHeight: 32, containerHeight: 600, scrollTop: 1000 * 32 });
    const b = computeVirtualRange({ itemCount: 50_000, itemHeight: 32, containerHeight: 600, scrollTop: 40_000 * 32 });
    expect(a.endIndex - a.startIndex).toBe(b.endIndex - b.startIndex);
  });
});
