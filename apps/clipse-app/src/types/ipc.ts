/**
 * Wire types shared with the daemon over the Tauri invoke boundary.
 *
 * These mirror `crates/clipse-core` and `crates/clipse-ipc` exactly, field for
 * field. None of the Rust structs involved carry `#[serde(rename_all = ...)]`
 * (ClipKind is the sole exception, called out below), so JSON keys are the
 * literal snake_case Rust field names — do not camelCase these, the daemon
 * will not recognise the result.
 */

// --- clipse-core: crates/clipse-core/src/clip.rs -----------------------

/** Tagged externally by serde's default enum representation: unit variants
 * serialize as a bare string, `Other(String)` as `{ "Other": "..." }`. */
export type ClipFormat =
  | "Text"
  | "Html"
  | "Rtf"
  | "Png"
  | "Jpeg"
  | "Svg"
  | "FileList"
  | { Other: string };

/** `#[serde(rename_all = "lowercase")]` — the one enum in this file that is
 * not PascalCase on the wire. */
export type ClipKind = "text" | "html" | "rtf" | "image" | "files" | "other";

/** `Inline` carries bytes as a plain JSON array of numbers (no
 * `serde_bytes`), `Blob` is a bare string meaning "not inline here". */
export type PayloadBody = "Blob" | { Inline: number[] };

export interface Payload {
  format: ClipFormat;
  /** Hex-encoded BLAKE3 digest (32 bytes -> 64 hex chars). */
  digest: string;
  size: number;
  body: PayloadBody;
}

export interface ClipSource {
  device: string;
  device_label: string;
  app: string | null;
}

// --- clipse-core: crates/clipse-core/src/hlc.rs -------------------------

export interface Hlc {
  wall_ms: number;
  counter: number;
  device: string;
}

// --- clipse-core: crates/clipse-core/src/clip.rs (Clip) -----------------

export interface Clip {
  id: string;
  hash: string;
  kind: ClipKind;
  payloads: Payload[];
  preview: string;
  source: ClipSource;
  hlc: Hlc;
  created_at_ms: number;
  pinned: boolean;
  deleted: boolean;
}

// --- clipse-ipc: crates/clipse-ipc/src/protocol.rs ----------------------

export interface HistoryQuery {
  limit: number;
  offset: number;
  kind: ClipKind | null;
  pinned_only: boolean;
}

export type CaptureMode = "Automatic" | { ManualPush: { reason: string } };

export interface DaemonStatus {
  device: string;
  device_label: string;
  daemon_version: string;
  paused: boolean;
  capture_mode: CaptureMode;
  clip_count: number;
  blob_bytes: number;
  blob_quota_bytes: number;
  peers_online: number;
  peers_total: number;
}

export type Connectivity = "Lan" | "Tailnet" | "Offline";

export interface PeerInfo {
  device: string;
  label: string;
  platform: string;
  connectivity: Connectivity;
  last_seen_ms: number | null;
}

export interface Settings {
  hotkey: string;
  apply_incoming_to_clipboard: boolean;
  blob_quota_bytes: number;
  blocked_apps: string[];
  detect_secrets: boolean;
  sync_enabled: boolean;
  /** mDNS announcement. Off means paired devices are only reachable at the
   * addresses recorded when they paired — nothing on the network is told this
   * machine runs Clipse. */
  announce_on_network: boolean;
  start_at_login: boolean;
  device_label: string;
}

// --- apps/clipse-app/src-tauri/src/commands.rs: CommandError ------------

/** `#[serde(tag = "kind", rename_all = "snake_case")]` on the Rust side. */
export type CommandError =
  | { kind: "not_connected" }
  | { kind: "daemon"; code: string; message: string }
  | { kind: "transport"; message: string };

export function isCommandError(value: unknown): value is CommandError {
  if (typeof value !== "object" || value === null || !("kind" in value)) {
    return false;
  }
  const kind = (value as { kind: unknown }).kind;
  return kind === "not_connected" || kind === "daemon" || kind === "transport";
}

/** The string a pairing screen shows, and when it stops being valid. */
export interface PairingOffer {
  uri: string;
  expires_at_ms: number;
  /** The offer as a self-contained SVG. Null when it could not be encoded, in
   * which case the copyable string is the whole story. */
  svg: string | null;
}

/** Both devices compute these. The user compares them. */
export interface PairingCode {
  digits: string;
  peer_label: string;
}
