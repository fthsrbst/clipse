/**
 * Two image encoders, written out by hand.
 *
 * Not because writing a PNG encoder is a good use of anybody's afternoon, but
 * because the alternative is a build-time image dependency for five files that
 * change once a year. The formats needed here are the two simplest ones there
 * are: an uncompressed 24-bit BMP, and a PNG whose only clever part is
 * delegated.
 *
 * **The BMP form is not a free choice.** NSIS and WiX both take bitmaps, and
 * both are old enough to be particular about them. 24-bit, uncompressed,
 * bottom-up rows padded to four bytes is the one shape every version accepts;
 * RLE and 32-bit-with-alpha are where installer bitmaps quietly fail to appear.
 *
 * Pure and browser-safe, like `ascii-raster.ts`. `toPng` takes its compressor
 * as an argument rather than importing `node:zlib`, so this file keeps no
 * platform of its own and the tests can hand it whatever they like.
 */

import type { Canvas } from "./ascii-raster";

/** Pixels per metre, both axes. 2835 is 72dpi, which is what every other
 * bitmap in the world claims and nothing actually reads. */
const RESOLUTION = 2835;

const BMP_FILE_HEADER = 14;
const BMP_INFO_HEADER = 40;

function u16(view: DataView, offset: number, value: number): void {
  view.setUint16(offset, value, true);
}

function u32(view: DataView, offset: number, value: number): void {
  view.setUint32(offset, value, true);
}

function i32(view: DataView, offset: number, value: number): void {
  view.setInt32(offset, value, true);
}

export function bmpRowStride(width: number): number {
  return (width * 3 + 3) & ~3;
}

export function toBmp24(source: Canvas): Uint8Array {
  const stride = bmpRowStride(source.width);
  const pixelBytes = stride * source.height;
  const out = new Uint8Array(BMP_FILE_HEADER + BMP_INFO_HEADER + pixelBytes);
  const view = new DataView(out.buffer);

  out[0] = 0x42; // B
  out[1] = 0x4d; // M
  u32(view, 2, out.length);
  u32(view, 6, 0);
  u32(view, 10, BMP_FILE_HEADER + BMP_INFO_HEADER);

  u32(view, 14, BMP_INFO_HEADER);
  i32(view, 18, source.width);
  // Positive height means the rows are stored bottom-up. Top-down BMPs are
  // legal and are exactly the ones that come out upside down in old readers.
  i32(view, 22, source.height);
  u16(view, 26, 1);
  u16(view, 28, 24);
  u32(view, 30, 0); // BI_RGB, no compression
  u32(view, 34, pixelBytes);
  i32(view, 38, RESOLUTION);
  i32(view, 42, RESOLUTION);
  u32(view, 46, 0);
  u32(view, 50, 0);

  const pixels = BMP_FILE_HEADER + BMP_INFO_HEADER;
  for (let y = 0; y < source.height; y++) {
    const sourceRow = (source.height - 1 - y) * source.width * 3;
    let target = pixels + y * stride;
    for (let x = 0; x < source.width; x++) {
      const i = sourceRow + x * 3;
      // BGR, not RGB.
      out[target++] = source.rgb[i + 2];
      out[target++] = source.rgb[i + 1];
      out[target++] = source.rgb[i];
    }
  }

  return out;
}

const CRC_TABLE = (() => {
  const table = new Uint32Array(256);
  for (let n = 0; n < 256; n++) {
    let c = n;
    for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
    table[n] = c >>> 0;
  }
  return table;
})();

export function crc32(data: Uint8Array): number {
  let c = 0xffffffff;
  for (let i = 0; i < data.length; i++) c = CRC_TABLE[(c ^ data[i]) & 0xff] ^ (c >>> 8);
  return (c ^ 0xffffffff) >>> 0;
}

const PNG_SIGNATURE = [0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a];

function chunk(type: string, body: Uint8Array): Uint8Array {
  const out = new Uint8Array(12 + body.length);
  const view = new DataView(out.buffer);
  view.setUint32(0, body.length, false);
  for (let i = 0; i < 4; i++) out[4 + i] = type.charCodeAt(i);
  out.set(body, 8);
  view.setUint32(8 + body.length, crc32(out.subarray(4, 8 + body.length)), false);
  return out;
}

function concat(parts: readonly Uint8Array[]): Uint8Array {
  const out = new Uint8Array(parts.reduce((n, p) => n + p.length, 0));
  let at = 0;
  for (const part of parts) {
    out.set(part, at);
    at += part.length;
  }
  return out;
}

/** A zlib stream over the raw scanlines — `zlib.deflateSync` in Node, which is
 * exactly the container PNG wants. */
export type Deflate = (data: Uint8Array) => Uint8Array;

export function toPng(source: Canvas, deflate: Deflate): Uint8Array {
  // Every scanline is prefixed with its filter type. Zero means "none": the
  // field is mostly flat black, which deflate handles well enough that
  // choosing filters per line would buy very little for a lot of code.
  const raw = new Uint8Array(source.height * (1 + source.width * 3));
  for (let y = 0; y < source.height; y++) {
    const target = y * (1 + source.width * 3);
    raw[target] = 0;
    raw.set(source.rgb.subarray(y * source.width * 3, (y + 1) * source.width * 3), target + 1);
  }

  const ihdr = new Uint8Array(13);
  const header = new DataView(ihdr.buffer);
  header.setUint32(0, source.width, false);
  header.setUint32(4, source.height, false);
  ihdr[8] = 8; // bit depth
  ihdr[9] = 2; // truecolour
  ihdr[10] = 0; // deflate
  ihdr[11] = 0; // adaptive filtering
  ihdr[12] = 0; // no interlacing

  return concat([
    Uint8Array.from(PNG_SIGNATURE),
    chunk("IHDR", ihdr),
    chunk("IDAT", deflate(raw)),
    chunk("IEND", new Uint8Array(0)),
  ]);
}
