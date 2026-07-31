import { describe, expect, it } from "vitest";

import {
  CELL_H,
  CELL_W,
  GLYPH_H,
  GLYPH_W,
  canvas,
  drawCells,
  drawField,
  fillRect,
  glyph,
  glyphCharacters,
  rampColours,
  setPixel,
  type Rgb,
} from "./ascii-raster";
import { RAMP } from "./eclipse-ascii";

const BLACK: Rgb = [0, 0, 0];
const WHITE: Rgb = [255, 255, 255];

function pixel(target: ReturnType<typeof canvas>, x: number, y: number): Rgb {
  const i = (y * target.width + x) * 3;
  return [target.rgb[i], target.rgb[i + 1], target.rgb[i + 2]];
}

describe("glyphs", () => {
  it("covers every character in the ramp, and nothing else", () => {
    expect(glyphCharacters().sort()).toEqual([...RAMP].sort());
  });

  /** Same reasoning as `ascii-logotype.test.ts`: a glyph one pixel short does
   * not fail loudly, it leans a character by a fraction of a cell. */
  it.each([...RAMP])("%s is a rectangle of the declared size", (char) => {
    const shape = glyph(char);
    expect(shape).toBeDefined();
    expect(shape).toHaveLength(GLYPH_H);
    for (const row of shape!) expect(row).toHaveLength(GLYPH_W);
  });

  it("fits every glyph inside its cell", () => {
    expect(GLYPH_W).toBeLessThanOrEqual(CELL_W);
    expect(GLYPH_H).toBeLessThanOrEqual(CELL_H);
  });

  /** The eclipse renderer corrects its circles for a cell twice as tall as it
   * is wide. Set in any other cell, the sun comes out an ellipse. */
  it("keeps the cell at the aspect the eclipse is drawn for", () => {
    expect(CELL_H).toBe(CELL_W * 2);
  });

  it("gives the dimmest ramp character less ink than the brightest", () => {
    const ink = (char: string) =>
      glyph(char)!.join("").split("").filter((c) => c === "#").length;
    expect(ink(RAMP[0])).toBeLessThan(ink(RAMP[RAMP.length - 1]));
  });
});

describe("canvas", () => {
  it("is filled with the given colour", () => {
    const target = canvas(3, 2, [1, 2, 3]);
    expect(target.rgb).toHaveLength(3 * 2 * 3);
    expect(pixel(target, 2, 1)).toEqual([1, 2, 3]);
  });

  /** Drawing at a negative offset is how the small header bitmaps are cropped
   * out of a much larger field, so out-of-bounds has to be a no-op rather than
   * a throw or a wrapped write. */
  it("ignores writes outside its bounds instead of wrapping", () => {
    const target = canvas(2, 2, BLACK);
    setPixel(target, -1, 0, WHITE);
    setPixel(target, 0, -1, WHITE);
    setPixel(target, 2, 0, WHITE);
    setPixel(target, 0, 2, WHITE);
    expect([...target.rgb]).toEqual(new Array(12).fill(0));
  });

  it("clips a rectangle at the edge", () => {
    const target = canvas(2, 2, BLACK);
    fillRect(target, 1, 1, 5, 5, WHITE);
    expect(pixel(target, 1, 1)).toEqual([255, 255, 255]);
    expect(pixel(target, 0, 0)).toEqual([0, 0, 0]);
  });
});

describe("rampColours", () => {
  it("starts and ends on the outer stops", () => {
    const colours = rampColours([[0, 0, 0], [255, 255, 255]], 12);
    expect(colours).toHaveLength(12);
    expect(colours[0]).toEqual([0, 0, 0]);
    expect(colours[11]).toEqual([255, 255, 255]);
  });

  /** Tone is carried by colour precisely because the glyphs are not monotonic
   * in ink. If this stops rising, the eclipse goes blotchy. */
  it("rises without a step backwards", () => {
    const colours = rampColours([[10, 0, 0], [100, 0, 0], [255, 0, 0]], 12);
    for (let i = 1; i < colours.length; i++) {
      expect(colours[i][0]).toBeGreaterThanOrEqual(colours[i - 1][0]);
    }
  });

  it("interpolates through an interior stop", () => {
    const colours = rampColours([[0, 0, 0], [10, 20, 30], [0, 0, 0]], 3);
    expect(colours[1]).toEqual([10, 20, 30]);
  });

  it("refuses a ramp it cannot interpolate", () => {
    expect(() => rampColours([[0, 0, 0]], 4)).toThrow();
    expect(() => rampColours([[0, 0, 0], [1, 1, 1]], 1)).toThrow();
  });
});

describe("drawField", () => {
  const colours = rampColours([[10, 10, 10], [250, 250, 250]], RAMP.length);
  const options = { ramp: RAMP, colours };

  it("draws a character at its cell, in the colour of its ramp position", () => {
    const target = canvas(CELL_W * 2, CELL_H, BLACK);
    // "." is a single dot at glyph row 5, column 2.
    drawField(target, [" ."], 0, 0, options);
    expect(pixel(target, CELL_W + 2, 3 + 5)).toEqual(colours[0]);
    expect(pixel(target, 2, 3 + 5)).toEqual([0, 0, 0]);
  });

  it("leaves a space genuinely empty", () => {
    const target = canvas(CELL_W, CELL_H, BLACK);
    drawField(target, [" "], 0, 0, options);
    expect([...target.rgb].every((b) => b === 0)).toBe(true);
  });

  it("crops rather than throws when drawn at a negative offset", () => {
    const target = canvas(CELL_W, CELL_H, BLACK);
    expect(() => drawField(target, ["@@@@", "@@@@"], -CELL_W, -CELL_H, options)).not.toThrow();
  });

  it("drops a cell when modulate returns a negative index", () => {
    const target = canvas(CELL_W, CELL_H, BLACK);
    drawField(target, ["@"], 0, 0, { ...options, modulate: () => -1 });
    expect([...target.rgb].every((b) => b === 0)).toBe(true);
  });

  it("draws the modulated ramp position, not the original one", () => {
    const target = canvas(CELL_W, CELL_H, BLACK);
    drawField(target, ["."], 0, 0, { ...options, modulate: () => RAMP.length - 1 });
    // "@" has a lit pixel where "." has none.
    expect(pixel(target, 1, 3)).toEqual(colours[RAMP.length - 1]);
  });

  it("ignores characters that are not in the ramp", () => {
    const target = canvas(CELL_W, CELL_H, BLACK);
    drawField(target, ["Z"], 0, 0, options);
    expect([...target.rgb].every((b) => b === 0)).toBe(true);
  });
});

describe("drawCells", () => {
  it("fills a whole cell per ink square", () => {
    const target = canvas(4, 2, BLACK);
    drawCells(target, ["# "], 0, 0, 2, 2, WHITE);
    expect(pixel(target, 0, 0)).toEqual([255, 255, 255]);
    expect(pixel(target, 1, 1)).toEqual([255, 255, 255]);
    expect(pixel(target, 2, 0)).toEqual([0, 0, 0]);
  });
});
