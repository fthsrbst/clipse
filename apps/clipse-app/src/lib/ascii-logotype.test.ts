import { describe, expect, it } from "vitest";

import { CLIPSE_WORDMARK, ECLIPSE_MARK } from "./ascii-logotype";

/** A character grid is only a drawing if every row is the same width. A short
 * row does not fail loudly — it leans the logo a fraction of a character, which
 * a human eye forgives for weeks and a test catches immediately. */
describe.each([
  ["ECLIPSE_MARK", ECLIPSE_MARK],
  ["CLIPSE_WORDMARK", CLIPSE_WORDMARK],
])("%s", (_name, grid) => {
  it("has rows", () => {
    expect(grid.length).toBeGreaterThan(0);
  });

  it("is rectangular", () => {
    const widths = new Set(grid.map((row) => row.length));
    expect([...widths]).toHaveLength(1);
  });

  it("carries no tab or newline, which would break the grid", () => {
    for (const row of grid) {
      expect(row).not.toMatch(/[\t\n\r]/);
    }
  });
});

describe("CLIPSE_WORDMARK", () => {
  it("is wider than it is tall, as a logotype for six letters must be", () => {
    expect(CLIPSE_WORDMARK[0].length).toBeGreaterThan(CLIPSE_WORDMARK.length * 3);
  });

  /** The counters are what stop a block letterform reading as a filled
   * rectangle at small sizes. If a letter loses its hole, the wordmark turns
   * into six smudges — so assert that each row band actually has interior
   * gaps rather than trusting the eye. */
  it("keeps interior gaps, so letters do not fill solid", () => {
    const interior = CLIPSE_WORDMARK.slice(1, -1);
    for (const row of interior) {
      expect(row.trim()).toMatch(/[#][ ]+[#]/);
    }
  });
});
