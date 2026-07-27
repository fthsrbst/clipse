import { expect, test, type Page } from "@playwright/test";
import { FIXTURE_CLIPS, FIXTURE_SETTINGS, FIXTURE_STATUS } from "./fixtures/clips";
import { installTauriStub, type TauriStubOptions } from "./fixtures/tauri-stub";

declare global {
  interface Window {
    __pasteCalls: string[];
    __hidePopupCalls: number;
  }
}

async function gotoPopup(page: Page) {
  await page.addInitScript<TauriStubOptions>(installTauriStub, {
    clips: FIXTURE_CLIPS,
    status: FIXTURE_STATUS,
    settings: FIXTURE_SETTINGS,
    windowLabel: "popup",
  });
  await page.goto("/");
  // Fixture clips load asynchronously on mount; wait for the first row.
  await expect(page.getByRole("option").first()).toBeVisible();
}

test("arrow keys move the selection through the visible list, wrapping at the ends", async ({ page }) => {
  await gotoPopup(page);
  const rows = page.getByRole("option");

  await expect(rows.nth(0)).toHaveAttribute("aria-selected", "true");

  await page.keyboard.press("ArrowDown");
  await expect(rows.nth(1)).toHaveAttribute("aria-selected", "true");
  await expect(rows.nth(0)).toHaveAttribute("aria-selected", "false");

  await page.keyboard.press("ArrowUp");
  await expect(rows.nth(0)).toHaveAttribute("aria-selected", "true");

  // Wrap from the first row back to the last on ArrowUp.
  await page.keyboard.press("ArrowUp");
  await expect(rows.nth(FIXTURE_CLIPS.length - 1)).toHaveAttribute("aria-selected", "true");
});

test("Enter fires the paste command for the selected clip and closes the popup", async ({ page }) => {
  await gotoPopup(page);

  await page.keyboard.press("ArrowDown");
  await page.keyboard.press("ArrowDown");
  const targetId = FIXTURE_CLIPS[2].id; // newest-first order matches fixture order

  await page.keyboard.press("Enter");

  await expect
    .poll(() => page.evaluate(() => window.__pasteCalls))
    .toEqual([targetId]);
  await expect.poll(() => page.evaluate(() => window.__hidePopupCalls)).toBe(1);
});

test("Ctrl+N pastes the Nth item directly without moving through it first", async ({ page }) => {
  await gotoPopup(page);
  const targetId = FIXTURE_CLIPS[3].id;

  await page.keyboard.press("Control+4");

  await expect
    .poll(() => page.evaluate(() => window.__pasteCalls))
    .toEqual([targetId]);
});

test("Escape closes the popup without pasting anything", async ({ page }) => {
  await gotoPopup(page);

  await page.keyboard.press("Escape");

  await expect.poll(() => page.evaluate(() => window.__hidePopupCalls)).toBe(1);
  expect(await page.evaluate(() => window.__pasteCalls)).toEqual([]);
});
