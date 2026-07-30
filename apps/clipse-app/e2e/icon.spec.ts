import { test } from "@playwright/test";

import { ECLIPSE_MARK } from "../src/lib/ascii-logotype";

/**
 * Renders the app icon from the same character grid the interface uses.
 *
 * Not a test — a generator, kept here because Playwright already has a browser
 * with the bundled DM Mono loaded, and drawing the mark with the real font is
 * the whole point. Run it, then feed the PNG to `pnpm tauri icon`.
 *
 * The mark degrades on purpose. At 1024px the characters are characters; at the
 * 32px Windows puts in a taskbar they blur into the ring they were always
 * drawing, which is the correct answer rather than a second mark.
 */
test("generate the app icon", async ({ page }) => {
  await page.setViewportSize({ width: 1024, height: 1024 });
  await page.goto("/");

  await page.setContent(
    `<!doctype html>
     <style>
       @font-face {
         font-family: "DM Mono";
         src: url("/fonts/dm-mono-latin-500-normal.woff2") format("woff2");
         font-weight: 500;
       }
       html, body { margin: 0; padding: 0; }
       body {
         width: 1024px;
         height: 1024px;
         display: grid;
         place-items: center;
         /* --void-950, the room the whole product is set in. */
         background: #06180F;
       }
       pre {
         margin: 0;
         font-family: "DM Mono", monospace;
         font-weight: 500;
         /* Sized so the ring clears the edges. An icon that touches its own
          * bounding box loses the ring to whatever rounds the corners. */
         font-size: 74px;
         line-height: 1;
         letter-spacing: 0;
         font-variant-ligatures: none;
         white-space: pre;
         color: #FB3640;
       }
     </style>
     <pre>${ECLIPSE_MARK.join("\n")}</pre>`,
  );

  // The font has to be in before the shot, or this renders in a fallback mono
  // whose advance width is different and whose circle is an egg.
  await page.evaluate(() => document.fonts.ready);
  await page.waitForTimeout(400);
  await page.screenshot({ path: "icon-source.png" });
});
