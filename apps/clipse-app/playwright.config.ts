import { defineConfig } from "@playwright/test";

// @ts-expect-error process is a nodejs global (no @types/node in this
// frontend-only package — see vite.config.ts for the same pattern).
const isCI = !!process.env.CI;

/**
 * Smoke suite against the plain Vite dev server, with the Tauri invoke
 * boundary stubbed per-test (see `e2e/fixtures/tauri-stub.ts`) — there is no
 * real `clipsed` daemon or Tauri runtime in this environment.
 */
export default defineConfig({
  testDir: "./e2e",
  fullyParallel: true,
  forbidOnly: isCI,
  retries: isCI ? 1 : 0,
  reporter: [["list"]],
  use: {
    baseURL: "http://localhost:1420",
    trace: "retain-on-failure",
  },
  webServer: {
    command: "pnpm run dev",
    url: "http://localhost:1420",
    reuseExistingServer: !isCI,
    timeout: 30_000,
  },
  projects: [{ name: "chromium", use: { browserName: "chromium" } }],
});
