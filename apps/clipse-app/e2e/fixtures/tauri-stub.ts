import type { Clip, DaemonStatus, Settings } from "../../src/types/ipc";

export interface TauriStubOptions {
  clips: Clip[];
  status: DaemonStatus;
  settings: Settings;
  /** Which Tauri window this page should present itself as — drives
   * `App.tsx`'s choice between the History window and the popup. */
  windowLabel: "main" | "popup";
  /** Digest → base64, standing in for the daemon's content-addressed store.
   * Without it a blob-backed payload has nowhere to come from. */
  blobs?: Record<string, string>;
  /**
   * Whether this page should behave as a first run.
   *
   * Defaults to "already introduced". Every browser context starts with an
   * empty localStorage, so without this the onboarding would stand in front of
   * every test that is actually about the history window — and a test suite
   * that has to click through a welcome sequence to reach its subject stops
   * testing the subject.
   */
  firstRun?: boolean;
}

/**
 * Installed via `page.addInitScript(installTauriStub, options)` *before*
 * the app's own scripts run, so `@tauri-apps/api/core`'s `invoke()` (which
 * reads `window.__TAURI_INTERNALS__.invoke`) finds a working transport on
 * first load rather than the real Tauri IPC bridge, which doesn't exist
 * when the suite runs against the plain Vite dev server.
 *
 * This function is serialized to a string and re-run inside the page, so it
 * must be self-contained — no closures over anything outside its own
 * parameter and browser globals.
 */
export function installTauriStub(options: TauriStubOptions): void {
  const win = window as unknown as {
    __TAURI_INTERNALS__?: Record<string, unknown>;
    __TAURI_EVENT_PLUGIN_INTERNALS__?: Record<string, unknown>;
    __pasteCalls: string[];
    __applyCalls: string[];
    __hidePopupCalls: number;
    isTauri?: boolean;
  };

  win.__pasteCalls = [];
  win.__applyCalls = [];
  win.__hidePopupCalls = 0;

  // Must match SEEN_KEY in src/components/onboarding.tsx.
  if (!options.firstRun) {
    try {
      window.localStorage.setItem("clipse.onboarding.seen.v1", "yes");
    } catch {
      /* A context that cannot store it will show the introduction; the test
       * asserting on the history window will then fail loudly, which is the
       * right outcome for a broken fixture. */
    }
  }

  let clips = options.clips.slice();
  let settings = { ...options.settings };
  const status = { ...options.status };

  function textOf(clip: Clip): string {
    const payload = clip.payloads.find((p) => p.format === "Text");
    if (!payload || payload.body === "Blob") return "";
    return new TextDecoder().decode(new Uint8Array(payload.body.Inline));
  }

  function page(list: Clip[], query: { limit?: number; offset?: number; kind?: string | null; pinned_only?: boolean }) {
    let filtered = list.filter((c) => !c.deleted);
    if (query.pinned_only) filtered = filtered.filter((c) => c.pinned);
    if (query.kind) filtered = filtered.filter((c) => c.kind === query.kind);
    filtered = filtered
      .slice()
      .sort((a, b) => b.hlc.wall_ms - a.hlc.wall_ms || b.hlc.counter - a.hlc.counter);
    const offset = query.offset ?? 0;
    const limit = query.limit && query.limit > 0 ? query.limit : filtered.length;
    return filtered.slice(offset, offset + limit);
  }

  async function invoke(cmd: string, args: Record<string, unknown> = {}): Promise<unknown> {
    switch (cmd) {
      case "plugin:event|listen":
        return Math.floor(Math.random() * 1e9);
      case "plugin:event|unlisten":
      case "plugin:event|emit":
        return null;

      case "history":
        return page(clips, (args.query as never) ?? {});
      case "search": {
        const needle = String(args.text ?? "").toLowerCase();
        const matched = clips.filter(
          (c) => c.preview.toLowerCase().includes(needle) || textOf(c).toLowerCase().includes(needle),
        );
        return page(matched, (args.query as never) ?? {});
      }
      case "get_clip":
        return clips.find((c) => c.id === args.id) ?? null;

      case "get_payload": {
        const clip = clips.find((c) => c.id === args.id);
        const payload = clip?.payloads.find((p) => p.format === args.format);
        if (!payload) return null;
        // A blob body carries no bytes; they come from the store, keyed by
        // digest, exactly as the daemon serves them. A digest with nothing
        // behind it answers null, which is the "too large to preview" path.
        if (payload.body === "Blob") return options.blobs?.[payload.digest] ?? null;
        let binary = "";
        for (const byte of payload.body.Inline) binary += String.fromCharCode(byte);
        return btoa(binary);
      }

      case "apply":
        win.__applyCalls.push(String(args.id));
        return null;
      case "paste":
        win.__pasteCalls.push(String(args.id));
        return null;

      case "set_pinned":
        clips = clips.map((c) => (c.id === args.id ? { ...c, pinned: Boolean(args.pinned) } : c));
        return null;
      case "delete":
        clips = clips.filter((c) => c.id !== args.id);
        return null;

      case "status":
        return { ...status, clip_count: clips.filter((c) => !c.deleted).length };
      case "set_paused":
        status.paused = Boolean(args.paused);
        return status;

      case "devices":
        return [];

      case "get_settings":
        return settings;
      case "update_settings":
        settings = args.settings as Settings;
        return settings;

      case "hide_popup":
        win.__hidePopupCalls += 1;
        return null;

      default:
        throw { kind: "transport", message: `unhandled fixture command: ${cmd}` };
    }
  }

  win.__TAURI_INTERNALS__ = win.__TAURI_INTERNALS__ ?? {};
  win.__TAURI_INTERNALS__.invoke = invoke;
  win.__TAURI_INTERNALS__.transformCallback = () => Math.floor(Math.random() * 1e9);
  win.__TAURI_INTERNALS__.unregisterCallback = () => {};
  win.__TAURI_INTERNALS__.metadata = {
    currentWindow: { label: options.windowLabel },
    currentWebview: { windowLabel: options.windowLabel, label: options.windowLabel },
  };
  win.__TAURI_EVENT_PLUGIN_INTERNALS__ = { unregisterListener: () => {} };
  win.isTauri = true;
}
