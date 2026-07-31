/**
 * Draws the five installer images and writes them to `assets/installer/`.
 *
 * Run it with `pnpm art`. The output is committed: regenerating is a command,
 * not a build step, so a release does not need a rasteriser and a change to the
 * artwork shows up in a diff rather than only in a built installer.
 *
 * This file is the only part of the artwork that touches a disk. Everything
 * with judgement in it — the glyphs, the ramp, the encoders — lives in
 * `src/lib/{ascii-raster,image-encode}.ts` where the tests can reach it.
 *
 * The compositions here answer to constraints, not to taste:
 *
 * - MSI paints its heading and body text straight onto `wix-dialog.bmp` in a
 *   fixed dark colour, and its page title onto the left of `wix-banner.bmp`.
 *   Those two images therefore have a light zone, and it is not optional: a
 *   uniformly black one produces an installer screen nobody can read.
 * - NSIS puts its text *beside* the sidebar, so that one can be fully dark.
 * - The DMG has no such constraint, and gets the whole field.
 */

import { deflateSync } from "node:zlib";
import { mkdirSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

import {
  CELL_H,
  CELL_W,
  canvas,
  drawCells,
  drawField,
  fillRect,
  rampColours,
  type Canvas,
  type Rgb,
} from "../src/lib/ascii-raster.ts";
import { RAMP, render } from "../src/lib/eclipse-ascii.ts";
import { CLIPSE_WORDMARK } from "../src/lib/ascii-logotype.ts";
import { toBmp24, toPng } from "../src/lib/image-encode.ts";

/* Read from `src/styles/tokens.css`. The amber values in
 * `assets/brand/tokens.css` are the superseded palette; the running eclipse is
 * red (`components/eclipse-canvas.module.css`). */
const VOID: Rgb = [0x02, 0x08, 0x06]; // --void-950
const SIGNAL_700: Rgb = [0x9e, 0x14, 0x1b];
const SIGNAL_500: Rgb = [0xfb, 0x36, 0x40];
const SIGNAL_300: Rgb = [0xff, 0x8a, 0x8f];
const LIT_100: Rgb = [0xe3, 0xeb, 0xe7];
const LIT_50: Rgb = [0xf4, 0xf8, 0xf6];

/** The wordmark is neutral rather than red on purpose. The room only reads as
 * black when something genuinely neutral is held next to it — a lesson this
 * project has now paid for twice. */
const WORDMARK: Rgb = LIT_100;

/**
 * The corona, dim to hot.
 *
 * It starts almost at the void and not at `--signal-700`, and that is the
 * difference between an eclipse and a rectangle of noise. `render` is a
 * full-bleed field — its corona reaches the edges of any frame you give it, by
 * construction — so the frame has to be made by tone. Set the dim end high
 * enough to see and the picture becomes a lit box with a ring in it.
 */
const CORONA = rampColours(
  [
    [0x18, 0x07, 0x09],
    [0x54, 0x0c, 0x11],
    SIGNAL_700,
    SIGNAL_500,
    SIGNAL_300,
  ],
  RAMP.length,
);

/** Totality. The disc is darkest on the page that is asking to be trusted. */
const TOTALITY = 0.5;

const HERE = dirname(fileURLToPath(import.meta.url));
const OUT = join(HERE, "..", "..", "..", "assets", "installer");

function field(cols: number, rows: number, phase = TOTALITY): string[] {
  return render({ width: cols, height: rows, phase, time: 0 });
}

/** Three pixels a cell. Set as glyphs at this size the wordmark is noise; set
 * as solid cells it is still the same grid and still a letterform. */
const MARK_CELL = 3;
const MARK_W = CLIPSE_WORDMARK[0].length * MARK_CELL;
const MARK_H = CLIPSE_WORDMARK.length * MARK_CELL;

/**
 * The dark panel both Windows welcome screens are built from: the eclipse
 * bleeding off the top, the wordmark below it, a rule at the foot.
 *
 * Shared rather than drawn twice because NSIS's sidebar and WiX's left band are
 * the same picture at two heights, and two copies would drift.
 */
function darkPanel(width: number, height: number): Canvas {
  const panel = canvas(width, height, VOID);

  const cols = Math.floor(width / CELL_W);
  // The field is full-bleed by design; the composition comes from where the
  // disc lands, not from a margin around it.
  const rows = Math.ceil((height - 96) / CELL_H);
  drawField(panel, field(cols, rows), Math.floor((width - cols * CELL_W) / 2), 0, {
    ramp: RAMP,
    colours: CORONA,
  });

  const markX = Math.floor((width - MARK_W) / 2);
  const markY = height - 58;
  drawCells(panel, CLIPSE_WORDMARK, markX, markY, MARK_CELL, MARK_CELL, WORDMARK);
  fillRect(panel, markX, markY + MARK_H + 14, MARK_W, 1, SIGNAL_700);

  return panel;
}

/**
 * The dark plate the two header strips are built from.
 *
 * Fifty-seven pixels is under five character rows: too few for an eclipse,
 * which needs a couple of dozen before it stops being four dots in a line. The
 * strips carry the wordmark instead, which is what a header is for, and the
 * eclipse stays on the two surfaces tall enough to hold it.
 */
function darkStrip(width: number, height: number): Canvas {
  const plate = canvas(width, height, VOID);

  const markX = Math.floor((width - MARK_W) / 2);
  const markY = Math.floor((height - MARK_H) / 2) - 3;
  drawCells(plate, CLIPSE_WORDMARK, markX, markY, MARK_CELL, MARK_CELL, WORDMARK);
  fillRect(plate, markX, markY + MARK_H + 6, MARK_W, 1, SIGNAL_700);

  return plate;
}

/* ── The five surfaces ─────────────────────────────────────────────────── */

/** NSIS Welcome and Finish, full left panel. Text sits beside it, so it may be
 * fully dark. */
function nsisSidebar(): Canvas {
  return darkPanel(164, 314);
}

/** NSIS header: the top-left plate on every page after Welcome, with the page
 * title set beside it. Also used by the uninstaller — an uninstaller that looks
 * like a different program is its own small alarm. */
function nsisHeader(): Canvas {
  return darkStrip(150, 57);
}

/**
 * The entire background of the MSI Welcome and Exit dialogs.
 *
 * The right two thirds are light because MSI draws black text over them. This
 * is the same division the stock WiX bitmap makes, and for the same reason.
 */
function wixDialog(): Canvas {
  const BAND = 165;
  const image = canvas(493, 312, LIT_50);
  const panel = darkPanel(BAND, 312);
  blit(image, panel, 0, 0);
  fillRect(image, BAND, 0, 1, 312, SIGNAL_700);
  return image;
}

/**
 * MSI's top strip, and the one surface that gets no dark field at all.
 *
 * WixUI writes the page title at 15 dialog units and its description at 25,
 * running up to 280 units wide — about 406 pixels once the 370-unit dialog is
 * scaled to this bitmap's 493. There is no room here for a band like the
 * dialog's without putting a black rectangle under grey body text. So the
 * banner inverts instead: the wordmark in void ink at the right edge, past
 * where any of that text can reach, over a rule that ties it to the rest.
 */
function wixBanner(): Canvas {
  const image = canvas(493, 58, LIT_50);

  const cell = 2;
  const width = CLIPSE_WORDMARK[0].length * cell;
  const x = 493 - width - 20;
  const y = Math.round((58 - CLIPSE_WORDMARK.length * cell) / 2);
  drawCells(image, CLIPSE_WORDMARK, x, y, cell, cell, VOID);

  fillRect(image, 0, 57, 493, 1, SIGNAL_500);
  return image;
}

/**
 * The Finder window the DMG opens.
 *
 * No arrow. The eclipse sits in the middle of the window and the two icons sit
 * either side of it, each in a clearing punched out of the corona — so the
 * gesture the window is asking for is a drag *across the eclipse*. Finder
 * supplies both labels; the drawing supplies the reason to look.
 *
 * The corona is also hotter along the line the icon travels, which is as much
 * direction as this needs. An arrow drawn on top would be the stock DMG, and
 * the stock DMG is the thing being replaced.
 *
 * The disc is not sizeable independently: `render` draws it at 29% of the
 * field's smaller dimension, so in a 420px-tall full-bleed field it is about
 * 244px across. The icon positions are chosen to clear it, not the reverse.
 *
 * The geometry here has to agree with `appPosition` and
 * `applicationFolderPosition` in `tauri.conf.json`. It has never been seen on a
 * Mac — see `docs/manual-verification.md`.
 */
const DMG = {
  width: 660,
  height: 420,
  appX: 104,
  folderX: 556,
  iconY: 210,
  /** A 128px icon plus its label, with room to breathe. */
  clearing: 86,
  fade: 24,
};

function dmgBackground(): Canvas {
  const image = canvas(DMG.width, DMG.height, VOID);

  const cols = Math.round(DMG.width / CELL_W);
  const rows = Math.round(DMG.height / CELL_H);

  const clearanceAt = (x: number, y: number, cx: number) => {
    const d = Math.hypot(x - cx, y - DMG.iconY);
    if (d <= DMG.clearing) return 0;
    if (d >= DMG.clearing + DMG.fade) return 1;
    return (d - DMG.clearing) / DMG.fade;
  };

  drawField(image, field(cols, rows), 0, 0, {
    ramp: RAMP,
    colours: CORONA,
    modulate: (col, row, index) => {
      const x = col * CELL_W + CELL_W / 2;
      const y = row * CELL_H + CELL_H / 2;

      const clear = Math.min(clearanceAt(x, y, DMG.appX), clearanceAt(x, y, DMG.folderX));
      if (clear === 0) return -1;

      const onTheLine = Math.max(0, 1 - Math.abs(y - DMG.iconY) / 40);
      return (index + 2 * onTheLine) * clear;
    },
  });

  return image;
}

function blit(target: Canvas, source: Canvas, x: number, y: number): void {
  for (let row = 0; row < source.height; row++) {
    for (let col = 0; col < source.width; col++) {
      const i = (row * source.width + col) * 3;
      const tx = x + col;
      const ty = y + row;
      if (tx < 0 || ty < 0 || tx >= target.width || ty >= target.height) continue;
      const t = (ty * target.width + tx) * 3;
      target.rgb[t] = source.rgb[i];
      target.rgb[t + 1] = source.rgb[i + 1];
      target.rgb[t + 2] = source.rgb[i + 2];
    }
  }
}

const OUTPUTS = [
  { path: ["windows", "nsis-sidebar.bmp"], draw: nsisSidebar, encode: "bmp" },
  { path: ["windows", "nsis-header.bmp"], draw: nsisHeader, encode: "bmp" },
  { path: ["windows", "wix-banner.bmp"], draw: wixBanner, encode: "bmp" },
  { path: ["windows", "wix-dialog.bmp"], draw: wixDialog, encode: "bmp" },
  { path: ["macos", "dmg-background.png"], draw: dmgBackground, encode: "png" },
] as const;

for (const output of OUTPUTS) {
  const image = output.draw();
  const bytes =
    output.encode === "bmp"
      ? toBmp24(image)
      : toPng(image, (data) => new Uint8Array(deflateSync(data)));

  const file = join(OUT, ...output.path);
  mkdirSync(dirname(file), { recursive: true });
  writeFileSync(file, bytes);
  console.log(`${output.path.join("/")}  ${image.width}×${image.height}  ${bytes.length} bytes`);
}
