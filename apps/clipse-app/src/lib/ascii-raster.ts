/**
 * The character grid, turned into pixels.
 *
 * Installers do not take text. NSIS wants a bitmap, WiX wants a bitmap, a DMG
 * wants a picture — so the eclipse has to be rasterised before it can appear on
 * any of them. This module is how, and it is deliberately the *only* place
 * where a character becomes a pixel.
 *
 * Everything here is pure and browser-safe: no `fs`, no `zlib`, no canvas. The
 * script that writes files (`scripts/render-installer-art.mts`) is a thin shell
 * over these functions, so the parts with judgement in them are the parts that
 * tests can reach.
 *
 * No font is involved. A font would mean a rasteriser, a rasteriser would mean
 * a dependency, and the twelve characters in `RAMP` are drawn here by hand for
 * the same reason `ascii-logotype.ts` draws six letters by hand: it is the only
 * way the installer's drawing and the running application's drawing are
 * provably the same drawing.
 */

/** Ink is `#`; everything else in a glyph row is transparent. */
const INK = "#";

export const GLYPH_W = 5;
export const GLYPH_H = 7;

/**
 * The cell a glyph is set in.
 *
 * Exactly twice as tall as it is wide, and that is not a taste decision:
 * `eclipse-ascii.ts` corrects its circles for a cell aspect of 0.5. Set the
 * same field in a cell of any other proportion and the sun comes out an
 * ellipse — subtly, which is worse than obviously.
 */
export const CELL_W = 6;
export const CELL_H = 12;

/** Glyph origin inside its cell. Off-centre downward, as type sits on a
 * baseline rather than in the middle of its line. */
const GLYPH_X = 0;
const GLYPH_Y = 3;

/**
 * The ramp, drawn.
 *
 * Ink roughly increases along `RAMP`, but not monotonically — a colon has
 * fewer lit pixels than a hyphen, and forcing it to have more would stop it
 * looking like a colon. Tone is carried by colour instead (see `rampColours`),
 * which is monotonic by construction. Shape here is texture, not brightness.
 */
const GLYPHS: Record<string, readonly string[]> = {
  ".": [".....", ".....", ".....", ".....", ".....", "..#..", "....."],
  ",": [".....", ".....", ".....", ".....", "..#..", "..#..", ".#..."],
  "-": [".....", ".....", ".....", ".###.", ".....", ".....", "....."],
  "~": [".....", ".....", ".##.#", "#..##", ".....", ".....", "....."],
  ":": [".....", ".....", "..#..", ".....", "..#..", ".....", "....."],
  ";": [".....", ".....", "..#..", ".....", "..#..", "..#..", ".#..."],
  "=": [".....", ".....", ".###.", ".....", ".###.", ".....", "....."],
  "!": ["..#..", "..#..", "..#..", "..#..", "..#..", ".....", "..#.."],
  "*": [".....", "..#..", "#.#.#", ".###.", "#.#.#", "..#..", "....."],
  "#": [".....", ".#.#.", "#####", ".#.#.", "#####", ".#.#.", "....."],
  "$": ["..#..", ".####", "#.#..", ".###.", "..#.#", "####.", "..#.."],
  "@": [".###.", "#...#", "#.##.", "#.#.#", "#.##.", "#....", ".###."],
};

export function glyph(char: string): readonly string[] | undefined {
  return GLYPHS[char];
}

export function glyphCharacters(): string[] {
  return Object.keys(GLYPHS);
}

export type Rgb = readonly [number, number, number];

export interface Canvas {
  readonly width: number;
  readonly height: number;
  /** Row-major, three bytes per pixel, no padding. */
  readonly rgb: Uint8Array;
}

export function canvas(width: number, height: number, fill: Rgb): Canvas {
  const rgb = new Uint8Array(width * height * 3);
  for (let i = 0; i < rgb.length; i += 3) {
    rgb[i] = fill[0];
    rgb[i + 1] = fill[1];
    rgb[i + 2] = fill[2];
  }
  return { width, height, rgb };
}

/** Silently ignores anything outside the canvas. That is what makes drawing at
 * a negative offset a crop rather than an error, which is how the small header
 * bitmaps are cut out of a full-size field. */
export function setPixel(target: Canvas, x: number, y: number, colour: Rgb): void {
  if (x < 0 || y < 0 || x >= target.width || y >= target.height) return;
  const i = (y * target.width + x) * 3;
  target.rgb[i] = colour[0];
  target.rgb[i + 1] = colour[1];
  target.rgb[i + 2] = colour[2];
}

export function fillRect(
  target: Canvas,
  x: number,
  y: number,
  width: number,
  height: number,
  colour: Rgb,
): void {
  for (let row = y; row < y + height; row++) {
    for (let col = x; col < x + width; col++) {
      setPixel(target, col, row, colour);
    }
  }
}

function lerp(a: number, b: number, t: number): number {
  return Math.round(a + (b - a) * t);
}

/**
 * A colour per ramp position, interpolated through the given stops.
 *
 * This is where brightness actually comes from. The glyphs supply texture; the
 * ramp index picks a colour that gets hotter along the string, so a field reads
 * as a gradient even where two adjacent characters happen to have similar ink.
 */
export function rampColours(stops: readonly Rgb[], count: number): Rgb[] {
  if (stops.length < 2) throw new Error("a ramp needs at least two stops");
  if (count < 2) throw new Error("a ramp needs at least two steps");

  return Array.from({ length: count }, (_, i) => {
    const position = (i / (count - 1)) * (stops.length - 1);
    const lower = Math.min(stops.length - 2, Math.floor(position));
    const t = position - lower;
    const from = stops[lower];
    const to = stops[lower + 1];
    return [lerp(from[0], to[0], t), lerp(from[1], to[1], t), lerp(from[2], to[2], t)] as Rgb;
  });
}

export interface FieldOptions {
  /** Ramp characters, dim to bright — pass `RAMP` from `eclipse-ascii`. */
  ramp: string;
  /** One colour per ramp position. */
  colours: readonly Rgb[];
  /** Cell size. Defaults to the 2:1 cell the eclipse is drawn for. */
  cellWidth?: number;
  cellHeight?: number;
  /**
   * Last say over every cell: given its grid position and its ramp index,
   * return the index to actually draw, or a negative number to drop the cell.
   *
   * This is how the DMG background clears a hole for an icon and brightens a
   * track between two of them without a second copy of the field maths.
   */
  modulate?: (col: number, row: number, index: number) => number;
}

/**
 * Draw a rendered field onto a canvas with its top-left cell at (x, y).
 *
 * `x` and `y` may be negative: the field is then cropped by the canvas edges,
 * which is how a 150×57 header bitmap is cut from a field far larger than it.
 */
export function drawField(
  target: Canvas,
  lines: readonly string[],
  x: number,
  y: number,
  options: FieldOptions,
): void {
  const cellW = options.cellWidth ?? CELL_W;
  const cellH = options.cellHeight ?? CELL_H;
  const last = options.colours.length - 1;

  for (let row = 0; row < lines.length; row++) {
    const line = lines[row];
    for (let col = 0; col < line.length; col++) {
      const char = line[col];
      let index = options.ramp.indexOf(char);
      if (index < 0) continue;

      if (options.modulate) {
        index = Math.round(options.modulate(col, row, index));
      }
      if (index < 0) continue;

      const colour = options.colours[Math.min(last, index)];
      const shape = GLYPHS[options.ramp[Math.min(options.ramp.length - 1, index)]];
      if (!shape) continue;

      const originX = x + col * cellW + GLYPH_X;
      const originY = y + row * cellH + GLYPH_Y;
      for (let gy = 0; gy < shape.length; gy++) {
        const glyphRow = shape[gy];
        for (let gx = 0; gx < glyphRow.length; gx++) {
          if (glyphRow[gx] === INK) setPixel(target, originX + gx, originY + gy, colour);
        }
      }
    }
  }
}

/**
 * Draw a character grid as solid cells rather than as glyphs.
 *
 * The wordmark is set this way. At the width of an NSIS sidebar its 35 columns
 * get about four pixels each, and a `#` drawn into four pixels is not a letter,
 * it is noise. Filling the cell keeps the same grid and stays a letterform.
 */
export function drawCells(
  target: Canvas,
  grid: readonly string[],
  x: number,
  y: number,
  cellWidth: number,
  cellHeight: number,
  colour: Rgb,
): void {
  for (let row = 0; row < grid.length; row++) {
    for (let col = 0; col < grid[row].length; col++) {
      if (grid[row][col] !== INK) continue;
      fillRect(target, x + col * cellWidth, y + row * cellHeight, cellWidth, cellHeight, colour);
    }
  }
}
