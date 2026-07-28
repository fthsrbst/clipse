import { expect, test } from "@playwright/test";
import { FIXTURE_CLIPS, FIXTURE_SETTINGS, FIXTURE_STATUS } from "./fixtures/clips";
import { installTauriStub, type TauriStubOptions } from "./fixtures/tauri-stub";

/**
 * The introduction is the first thing anyone sees, so what matters here is not
 * that the four screens render — it is that nobody gets trapped in them and
 * nobody is shown them twice.
 */
test.beforeEach(async ({ page }) => {
  await page.addInitScript<TauriStubOptions>(installTauriStub, {
    clips: FIXTURE_CLIPS,
    status: FIXTURE_STATUS,
    settings: FIXTURE_SETTINGS,
    windowLabel: "main",
    firstRun: true,
  });
  await page.goto("/");
});

test("a first run opens on the introduction rather than the history", async ({ page }) => {
  await expect(page.getByRole("heading", { name: "Everything you copy, kept." })).toBeVisible();
  await expect(page.getByRole("option")).toHaveCount(0);
});

test("Next walks forward through every screen and ends on the history", async ({ page }) => {
  await expect(page.getByRole("heading", { name: "Everything you copy, kept." })).toBeVisible();

  await page.getByRole("button", { name: "Next" }).click();
  await expect(
    page.getByRole("heading", { name: "Some things are never written down." }),
  ).toBeVisible();

  await page.getByRole("button", { name: "Next" }).click();
  await expect(page.getByRole("heading", { name: "Your devices, and nobody else's." })).toBeVisible();

  await page.getByRole("button", { name: "Next" }).click();
  await expect(page.getByRole("heading", { name: "One shortcut, anywhere." })).toBeVisible();

  // The last step advertises the real hotkey, humanised — not the stored
  // `CmdOrCtrl+…` accelerator.
  await expect(page.getByText("Ctrl + Shift + V")).toBeVisible();

  await page.getByRole("button", { name: "Start" }).click();
  await expect(page.getByRole("option")).toHaveCount(FIXTURE_CLIPS.length);
});

test("Back returns to the previous screen", async ({ page }) => {
  await page.getByRole("button", { name: "Next" }).click();
  await expect(
    page.getByRole("heading", { name: "Some things are never written down." }),
  ).toBeVisible();

  await page.getByRole("button", { name: "Back" }).click();
  await expect(page.getByRole("heading", { name: "Everything you copy, kept." })).toBeVisible();

  // Nothing to go back to on the first screen, so the control is not offered.
  await expect(page.getByRole("button", { name: "Back" })).toHaveCount(0);
});

test("Skip goes straight to the history", async ({ page }) => {
  await page.getByRole("button", { name: "Skip" }).click();
  await expect(page.getByRole("option")).toHaveCount(FIXTURE_CLIPS.length);
});

test("the introduction is not shown again after it has been finished", async ({ page }) => {
  await page.getByRole("button", { name: "Skip" }).click();
  await expect(page.getByRole("option")).toHaveCount(FIXTURE_CLIPS.length);

  // A reload is the closest thing to reopening the window: the stub reinstalls
  // itself, but localStorage survives, which is exactly what is being tested.
  await page.reload();
  await expect(page.getByRole("option")).toHaveCount(FIXTURE_CLIPS.length);
  await expect(page.getByRole("heading", { name: "Everything you copy, kept." })).toHaveCount(0);
});

test("the arrow keys and Escape drive it too", async ({ page }) => {
  await page.keyboard.press("ArrowRight");
  await expect(
    page.getByRole("heading", { name: "Some things are never written down." }),
  ).toBeVisible();

  await page.keyboard.press("ArrowLeft");
  await expect(page.getByRole("heading", { name: "Everything you copy, kept." })).toBeVisible();

  await page.keyboard.press("Escape");
  await expect(page.getByRole("option")).toHaveCount(FIXTURE_CLIPS.length);
});
