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

test("the footer reports the daemon's clip count", async ({ page }) => {
  await expect(page.getByText(`${FIXTURE_CLIPS.length} clips`)).toBeVisible();
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
