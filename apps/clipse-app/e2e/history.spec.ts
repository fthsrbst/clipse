import { expect, test } from "@playwright/test";
import { FIXTURE_CLIPS, FIXTURE_SETTINGS, FIXTURE_STATUS } from "./fixtures/clips";
import { installTauriStub, type TauriStubOptions } from "./fixtures/tauri-stub";

test.beforeEach(async ({ page }) => {
  await page.addInitScript<TauriStubOptions>(installTauriStub, {
    clips: FIXTURE_CLIPS,
    status: FIXTURE_STATUS,
    settings: FIXTURE_SETTINGS,
    windowLabel: "main",
  });
  await page.goto("/");
});

test("history window renders every fixture clip", async ({ page }) => {
  const rows = page.getByRole("option");
  await expect(rows).toHaveCount(FIXTURE_CLIPS.length);
  await expect(page.getByText("Meeting notes: ship F1 before the offsite.")).toBeVisible();
  await expect(page.getByText("Grocery list: milk, eggs, bread")).toBeVisible();
});

// The count lives in the spine now, set as display type, with the noun beside
// it rather than in the same string — so it is two elements to assert on
// instead of one phrase.
test("the spine reports the clip count", async ({ page }) => {
  await expect(page.getByText(String(FIXTURE_CLIPS.length), { exact: true })).toBeVisible();
  await expect(page.getByText("clips", { exact: true })).toBeVisible();
});

test("the spine carries the identity as text, not as an image", async ({ page }) => {
  // The whole point of the ASCII identity is that it is characters. An <img> or
  // an inline <svg> would look identical in a screenshot and be a silent
  // regression, so assert on the text content.
  const logo = page.getByRole("img", { name: "Clipse" });
  await expect(logo).toBeVisible();
  await expect(logo).toContainText("#");
});

// The frame has no title bar of its own and no OS one either, so these are the
// only way to minimise or close. If they ever stop rendering, the window
// becomes uncloseable rather than merely ugly.
test("the frame carries its own window controls", async ({ page }) => {
  await expect(page.getByRole("button", { name: "Minimise" })).toBeVisible();
  await expect(page.getByRole("button", { name: "Maximise" })).toBeVisible();
  await expect(page.getByRole("button", { name: "Close" })).toBeVisible();
});

test("the refused-secrets count is reported without any content", async ({ page }) => {
  await expect(page.getByText("refused", { exact: true })).toBeVisible();
  await expect(page.getByText(String(FIXTURE_STATUS.secrets_refused), { exact: true })).toBeVisible();
});

test("settings keeps the spine mounted and returns on Escape", async ({ page }) => {
  await page.getByRole("button", { name: "Settings" }).click();
  await expect(page.getByText("Settings", { exact: true })).toBeVisible();

  // Not a separate screen: the rail it was opened from is still there, and the
  // control that opened it reads as pressed — which is what removes the need
  // for a back button.
  await expect(page.getByRole("button", { name: "Settings" })).toHaveAttribute(
    "aria-pressed",
    "true",
  );
  await expect(page.getByRole("button", { name: "← Back" })).toHaveCount(0);

  await page.keyboard.press("Escape");
  await expect(page.getByRole("listbox", { name: "Clipboard history" })).toBeVisible();
});

test("search narrows the list to matching clips", async ({ page }) => {
  await page.getByRole("textbox", { name: "Search clipboard history" }).fill("grocery");

  await expect(page.getByRole("option")).toHaveCount(1);
  await expect(page.getByText("Grocery list: milk, eggs, bread")).toBeVisible();
  await expect(page.getByText("Meeting notes: ship F1 before the offsite.")).not.toBeVisible();
});

test("clearing the search restores the full list", async ({ page }) => {
  const search = page.getByRole("textbox", { name: "Search clipboard history" });
  await search.fill("grocery");
  await expect(page.getByRole("option")).toHaveCount(1);

  await search.fill("");
  await expect(page.getByRole("option")).toHaveCount(FIXTURE_CLIPS.length);
});

test("a search with no matches shows the empty state, not a blank list", async ({ page }) => {
  await page.getByRole("textbox", { name: "Search clipboard history" }).fill("nothing matches this at all");
  await expect(page.getByRole("option")).toHaveCount(0);
  await expect(page.getByText("No matches")).toBeVisible();
});
