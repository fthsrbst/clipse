/**
 * Fixture data for the Playwright smoke suite, shaped exactly like the wire
 * format in `src/types/ipc.ts` (which mirrors `crates/clipse-core` /
 * `crates/clipse-ipc`). This file runs under Node (inside the Playwright
 * test process), not the browser — see `tauri-stub.ts` for how it gets
 * injected into the page.
 */
import type { Clip, DaemonStatus, Settings } from "../../src/types/ipc";

const DEVICE_A = "11111111-1111-4111-8111-111111111111";
const DEVICE_B = "22222222-2222-4222-8222-222222222222";

function textPayload(text: string) {
  const bytes = Array.from(new TextEncoder().encode(text));
  return {
    format: "Text" as const,
    digest: "0".repeat(64),
    size: bytes.length,
    body: { Inline: bytes },
  };
}

// A 1x1 transparent PNG, small enough to stay comfortably inline.
const TINY_PNG_BASE64 =
  "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNk+A8AAQUBAScY42YAAAAASUVORK5CYII=";

/** `atob`, not `Buffer` — this file is imported by both the Playwright
 * (Node) test process and, indirectly, gets serialized alongside browser
 * code, so it sticks to globals available in both rather than adding
 * `@types/node` for one `Buffer.from` call. */
function base64ToBytes(base64: string): number[] {
  const binary = atob(base64);
  return Array.from(binary, (char) => char.charCodeAt(0));
}

function pngPayload() {
  const bytes = base64ToBytes(TINY_PNG_BASE64);
  return {
    format: "Png" as const,
    digest: "1".repeat(64),
    size: bytes.length,
    body: { Inline: bytes },
  };
}

function fileListPayload(paths: string[]) {
  const bytes = Array.from(new TextEncoder().encode(paths.join("\n")));
  return {
    format: "FileList" as const,
    digest: "2".repeat(64),
    size: bytes.length,
    body: { Inline: bytes },
  };
}

function source(device: string, label: string) {
  return { device, device_label: label, app: null };
}

function hlc(wallMs: number, device: string) {
  return { wall_ms: wallMs, counter: 0, device };
}

const NOW = Date.parse("2026-07-27T12:00:00.000Z");

export const FIXTURE_CLIPS: Clip[] = [
  {
    id: "aaaaaaaa-0000-4000-8000-000000000001",
    hash: "a".repeat(64),
    kind: "text",
    payloads: [textPayload("Meeting notes: ship F1 before the offsite.")],
    preview: "Meeting notes: ship F1 before the offsite.",
    source: source(DEVICE_A, "MacBook Pro"),
    hlc: hlc(NOW - 5 * 60_000, DEVICE_A),
    created_at_ms: NOW - 5 * 60_000,
    pinned: false,
    deleted: false,
  },
  {
    id: "aaaaaaaa-0000-4000-8000-000000000002",
    hash: "b".repeat(64),
    kind: "text",
    payloads: [textPayload("https://clipse.dev")],
    preview: "https://clipse.dev",
    source: source(DEVICE_A, "MacBook Pro"),
    hlc: hlc(NOW - 60 * 60_000, DEVICE_A),
    created_at_ms: NOW - 60 * 60_000,
    pinned: false,
    deleted: false,
  },
  {
    id: "aaaaaaaa-0000-4000-8000-000000000003",
    hash: "c".repeat(64),
    kind: "image",
    payloads: [pngPayload()],
    preview: "Image · 68 B",
    source: source(DEVICE_B, "Desktop PC"),
    hlc: hlc(NOW - 2 * 60 * 60_000, DEVICE_B),
    created_at_ms: NOW - 2 * 60 * 60_000,
    pinned: true,
    deleted: false,
  },
  {
    id: "aaaaaaaa-0000-4000-8000-000000000004",
    hash: "d".repeat(64),
    kind: "files",
    payloads: [fileListPayload(["/Users/fatih/report.pdf"])],
    preview: "1 file(s)",
    source: source(DEVICE_A, "MacBook Pro"),
    hlc: hlc(NOW - 3 * 60 * 60_000, DEVICE_A),
    created_at_ms: NOW - 3 * 60 * 60_000,
    pinned: false,
    deleted: false,
  },
  {
    id: "aaaaaaaa-0000-4000-8000-000000000005",
    hash: "e".repeat(64),
    kind: "text",
    payloads: [textPayload("Grocery list: milk, eggs, bread")],
    preview: "Grocery list: milk, eggs, bread",
    source: source(DEVICE_B, "Desktop PC"),
    hlc: hlc(NOW - 4 * 60 * 60_000, DEVICE_B),
    created_at_ms: NOW - 4 * 60 * 60_000,
    pinned: false,
    deleted: false,
  },
];

export const FIXTURE_STATUS: DaemonStatus = {
  device: DEVICE_A,
  device_label: "MacBook Pro",
  daemon_version: "fixture-0.0.0",
  paused: false,
  capture_mode: "Automatic",
  clip_count: FIXTURE_CLIPS.length,
  blob_bytes: 68,
  blob_quota_bytes: 2 * 1024 * 1024 * 1024,
  peers_online: 0,
  peers_total: 0,
  // Non-zero on purpose: the spine renders this, and a fixture of 0 would let
  // a broken readout pass for an empty one.
  secrets_refused: 2,
};

export const FIXTURE_SETTINGS: Settings = {
  hotkey: "CmdOrCtrl+Shift+V",
  apply_incoming_to_clipboard: true,
  blob_quota_bytes: 2 * 1024 * 1024 * 1024,
  blocked_apps: [],
  detect_secrets: true,
  sync_enabled: true,
  announce_on_network: true,
  start_at_login: true,
  device_label: "MacBook Pro",
};
