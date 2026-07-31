import { describe, expect, it } from "vitest";

import { canvas, setPixel, type Rgb } from "./ascii-raster";
import { bmpRowStride, crc32, toBmp24, toPng } from "./image-encode";

const RED: Rgb = [251, 54, 64];

/**
 * `toPng` takes its compressor as an argument, so the tests hand it one that
 * compresses nothing.
 *
 * That is not a shortcut around zlib — it is the point of the injection. What
 * this file is responsible for is the scanlines it builds and the chunks it
 * wraps them in; whether deflate works is deflate's problem. Handing it the
 * identity makes the payload directly readable, and the real compressor is
 * exercised where it matters, by `pnpm art` writing a file that opens.
 */
const verbatim = (data: Uint8Array) => data;

describe("bmpRowStride", () => {
  /** A row that is not a multiple of four bytes is where hand-written BMPs go
   * wrong, and the failure is a picture sheared diagonally rather than an
   * error. */
  it.each([
    [1, 4],
    [2, 8],
    [3, 12],
    [4, 12],
    [150, 452],
    [164, 492],
    [493, 1480],
  ])("pads a %i-pixel row to %i bytes", (width, stride) => {
    expect(bmpRowStride(width)).toBe(stride);
    expect(stride % 4).toBe(0);
  });
});

describe("toBmp24", () => {
  it("writes a header NSIS and WiX will accept", () => {
    const bmp = toBmp24(canvas(164, 314, [0, 0, 0]));
    const view = new DataView(bmp.buffer, bmp.byteOffset, bmp.byteLength);

    expect(bmp[0]).toBe(0x42);
    expect(bmp[1]).toBe(0x4d);
    expect(view.getUint32(2, true)).toBe(bmp.length);
    expect(view.getUint32(10, true)).toBe(54); // pixel data offset
    expect(view.getUint32(14, true)).toBe(40); // BITMAPINFOHEADER
    expect(view.getInt32(18, true)).toBe(164);
    // Positive: bottom-up. Top-down BMPs are the ones old readers invert.
    expect(view.getInt32(22, true)).toBe(314);
    expect(view.getUint16(28, true)).toBe(24);
    expect(view.getUint32(30, true)).toBe(0); // BI_RGB, uncompressed
  });

  it("is exactly as long as its header claims", () => {
    const bmp = toBmp24(canvas(7, 5, [0, 0, 0]));
    expect(bmp.length).toBe(54 + bmpRowStride(7) * 5);
  });

  it("stores rows bottom-up and channels as BGR", () => {
    const source = canvas(2, 2, [0, 0, 0]);
    setPixel(source, 0, 0, RED); // top-left
    const bmp = toBmp24(source);

    const stride = bmpRowStride(2);
    // The top row is stored last.
    const topRow = 54 + stride * 1;
    expect([bmp[topRow], bmp[topRow + 1], bmp[topRow + 2]]).toEqual([RED[2], RED[1], RED[0]]);
    expect(bmp[54]).toBe(0); // bottom row is untouched
  });
});

describe("crc32", () => {
  it("matches the known value for the PNG IEND chunk type", () => {
    expect(crc32(Uint8Array.from([0x49, 0x45, 0x4e, 0x44]))).toBe(0xae426082);
  });

  it("matches the known value for an empty input", () => {
    expect(crc32(new Uint8Array(0))).toBe(0);
  });
});

describe("toPng", () => {
  it("writes a signature, an IHDR that describes the canvas, and an IEND", () => {
    const png = toPng(canvas(660, 420, [2, 8, 6]), verbatim);
    expect([...png.subarray(0, 8)]).toEqual([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]);

    const view = new DataView(png.buffer, png.byteOffset, png.byteLength);
    expect(view.getUint32(8, false)).toBe(13); // IHDR length
    expect(String.fromCharCode(...png.subarray(12, 16))).toBe("IHDR");
    expect(view.getUint32(16, false)).toBe(660);
    expect(view.getUint32(20, false)).toBe(420);
    expect(png[24]).toBe(8); // bit depth
    expect(png[25]).toBe(2); // truecolour
    expect(String.fromCharCode(...png.subarray(png.length - 8, png.length - 4))).toBe("IEND");
  });

  it("hands the compressor filtered scanlines, in order", () => {
    const source = canvas(3, 2, [1, 2, 3]);
    setPixel(source, 2, 1, RED);
    const png = toPng(source, verbatim);

    // IDAT begins after the signature and the 25-byte IHDR chunk.
    const view = new DataView(png.buffer, png.byteOffset, png.byteLength);
    const length = view.getUint32(33, false);
    expect(String.fromCharCode(...png.subarray(37, 41))).toBe("IDAT");
    const raw = png.subarray(41, 41 + length);

    expect(raw).toHaveLength(2 * (1 + 3 * 3));
    expect(raw[0]).toBe(0); // filter: none
    // Second row, third pixel: past row 0 (ten bytes), past row 1's filter byte
    // and its first two pixels.
    expect([...raw.subarray(10 + 1 + 6, 10 + 1 + 9)]).toEqual([251, 54, 64]);
  });

  it("carries a valid CRC on every chunk", () => {
    const png = toPng(canvas(4, 4, [0, 0, 0]), verbatim);
    const view = new DataView(png.buffer, png.byteOffset, png.byteLength);
    let at = 8;
    let chunks = 0;
    while (at < png.length) {
      const length = view.getUint32(at, false);
      const end = at + 8 + length;
      expect(view.getUint32(end, false)).toBe(crc32(png.subarray(at + 4, end)));
      at = end + 4;
      chunks++;
    }
    expect(chunks).toBe(3);
    expect(at).toBe(png.length);
  });
});
