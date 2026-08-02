/**
 * Thin, typed wrapper around the Tauri invoke boundary.
 *
 * Every function here corresponds 1:1 to a command registered in
 * `src-tauri/src/lib.rs`'s `invoke_handler!` — see `src-tauri/src/commands.rs`
 * for the Rust side. Nothing in this file talks to the daemon directly; it
 * only talks to the Tauri command layer, which does that.
 */

import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";

import {
  type Clip,
  type ClipFormat,
  type CommandError,
  type DaemonStatus,
  type HistoryQuery,
  type PeerInfo,
  type Paired,
  type PairingCode,
  type Settings,
  isCommandError,
} from "../types/ipc";

/** Thrown by every wrapper below in place of the raw invoke rejection, so
 * callers can pattern-match on `.detail.kind` instead of re-parsing an
 * `unknown` error. */
export class DaemonError extends Error {
  readonly detail: CommandError;

  constructor(detail: CommandError) {
    super(describeCommandError(detail));
    this.name = "DaemonError";
    this.detail = detail;
  }
}

function describeCommandError(err: CommandError): string {
  switch (err.kind) {
    case "not_connected":
      return "Clipse daemon is not running.";
    case "daemon":
      return `${err.code}: ${err.message}`;
    case "transport":
      return err.message;
  }
}

async function call<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  try {
    return await invoke<T>(cmd, args);
  } catch (err) {
    if (isCommandError(err)) throw new DaemonError(err);
    throw err;
  }
}

export function isNotConnected(err: unknown): boolean {
  return err instanceof DaemonError && err.detail.kind === "not_connected";
}

export const api = {
  history: (query: HistoryQuery) => call<Clip[]>("history", { query }),
  search: (text: string, query: HistoryQuery) => call<Clip[]>("search", { text, query }),
  getClip: (id: string) => call<Clip | null>("get_clip", { id }),
  /** Base64 of one payload, or `null` when the clip, the format, or a size
   * under the daemon's 24MB preview cap is missing. All three are ordinary
   * answers rather than errors — the caller shows a size instead. */
  getPayload: (id: string, format: ClipFormat) =>
    call<string | null>("get_payload", { id, format }),
  apply: (id: string) => call<void>("apply", { id }),
  paste: (id: string) => call<void>("paste", { id }),
  setPinned: (id: string, pinned: boolean) => call<void>("set_pinned", { id, pinned }),
  delete: (id: string) => call<void>("delete", { id }),
  status: () => call<DaemonStatus>("status"),
  setPaused: (paused: boolean) => call<void>("set_paused", { paused }),
  devices: () => call<PeerInfo[]>("devices"),

  beginPairing: () => call<PairingCode>("begin_pairing"),
  /** The six digits read off the other screen. Resolves once the two devices
   * have verified each other, rejects if nothing on the network is showing
   * that code — there is no step in between for the user to get wrong. */
  pairWithCode: (code: string) => call<Paired>("pair_with_code", { code }),
  cancelPairing: () => call<void>("cancel_pairing"),
  forgetDevice: (device: string) => call<void>("forget_device", { device }),
  getSettings: () => call<Settings>("get_settings"),
  updateSettings: (settings: Settings) => call<Settings>("update_settings", { settings }),
  hidePopup: () => call<void>("hide_popup"),
};

/** Which Tauri window this JS context is running in ("main" or "popup"),
 * from `src-tauri/tauri.conf.json`. Falls back to "main" outside a Tauri
 * runtime (e.g. `vite dev` opened directly in a browser). */
export function currentWindowLabel(): string {
  try {
    return getCurrentWindow().label;
  } catch {
    return "main";
  }
}

// --- Events pushed from src-tauri/src/connection.rs ---------------------

/**
 * The hotkey popup has just been shown.
 *
 * Emitted from `src-tauri/src/popup.rs` rather than derived in the webview,
 * because the webview is hidden between uses instead of destroyed: there is no
 * mount to hook, and nothing else tells it that it is on screen again.
 */
export function onPopupShown(handler: () => void) {
  return listen<null>("popup:shown", () => handler());
}

export function onClipAdded(handler: (clip: Clip) => void) {
  return listen<Clip>("clip-added", (e) => handler(e.payload));
}

export function onClipUpdated(handler: (clip: Clip) => void) {
  return listen<Clip>("clip-updated", (e) => handler(e.payload));
}

export function onClipRemoved(handler: (id: string) => void) {
  return listen<string>("clip-removed", (e) => handler(e.payload));
}

export function onStatusChanged(handler: (status: DaemonStatus) => void) {
  return listen<DaemonStatus>("status-changed", (e) => handler(e.payload));
}

export function onDeviceChanged(handler: (peer: PeerInfo) => void) {
  return listen<PeerInfo>("device-changed", (e) => handler(e.payload));
}

/** The whole paired-device list, pushed after every (re)connect. */
export function onDevicesChanged(handler: (peers: PeerInfo[]) => void) {
  return listen<PeerInfo[]>("devices-changed", (e) => handler(e.payload));
}

export function onSuppressed(handler: (reason: string) => void) {
  return listen<string>("suppressed", (e) => handler(e.payload));
}

/** Fires on the device that showed the digits, once the ceremony completes.
 * That device is not the one that typed anything, so this is the only way its
 * screen learns it worked. */
export function onPairingSucceeded(handler: (peerLabel: string) => void) {
  return listen<string>("pairing-succeeded", (e) => handler(e.payload));
}

export function onPairingEnded(handler: (reason: string) => void) {
  return listen<string>("pairing-ended", (e) => handler(e.payload));
}

export function onConnectionChanged(handler: (connected: boolean) => void) {
  return listen<boolean>("connection-changed", (e) => handler(e.payload));
}
