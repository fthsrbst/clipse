/** Helpers for reading a `Clip`'s payloads and mapping it onto the UI's
 * coarser type-filter categories. Kept separate from `types/ipc.ts` (data
 * shapes only) and `tauri-client.ts` (IPC only). */

import type { ClipTypeFilter } from "./popup-reducer";
import type { Clip, ClipFormat, ClipKind, Payload } from "../types/ipc";

function formatLabel(format: ClipFormat): string {
  return typeof format === "string" ? format : "Other";
}

export function findPayload(clip: Clip, format: "Text" | "Png" | "Jpeg" | "Svg"): Payload | undefined {
  return clip.payloads.find((p) => formatLabel(p.format) === format);
}

function inlineBytes(payload: Payload): Uint8Array | null {
  if (payload.body === "Blob") return null;
  return Uint8Array.from(payload.body.Inline);
}

/** Plain-text content of a clip, when it has a text payload that is stored
 * inline (payloads above 64KB live in the blob store and are not fetchable
 * from this frontend). */
export function getClipText(clip: Clip): string | undefined {
  const payload = findPayload(clip, "Text");
  if (!payload) return undefined;
  const bytes = inlineBytes(payload);
  if (!bytes) return undefined;
  return new TextDecoder().decode(bytes);
}

function bytesToBase64(bytes: Uint8Array): string {
  let binary = "";
  const chunkSize = 0x8000;
  for (let i = 0; i < bytes.length; i += chunkSize) {
    binary += String.fromCharCode(...bytes.subarray(i, i + chunkSize));
  }
  return btoa(binary);
}

const IMAGE_MIME: Record<string, string> = {
  Png: "image/png",
  Jpeg: "image/jpeg",
  Svg: "image/svg+xml",
};

/** The MIME type for a format the UI renders as an image, or `null` for
 * everything else. */
export function mimeForFormat(format: ClipFormat): string | null {
  const label = formatLabel(format);
  return IMAGE_MIME[label] ?? null;
}

/** A `data:` URL from base64 the daemon returned for a blob-backed payload.
 * `null` when the format is not one that can be shown as an image. */
export function payloadDataUrl(format: ClipFormat, base64: string): string | null {
  const mime = mimeForFormat(format);
  return mime ? `data:${mime};base64,${base64}` : null;
}

/** A `data:` URL for a clip's image payload, if it has one and it is stored
 * inline. Returns `undefined` for a blob-backed image (large screenshot) —
 * callers should fetch it with `api.getPayload` or show a size-only
 * placeholder. */
export function getClipImageDataUrl(clip: Clip): string | undefined {
  for (const kind of ["Png", "Jpeg", "Svg"] as const) {
    const payload = findPayload(clip, kind);
    if (!payload) continue;
    const bytes = inlineBytes(payload);
    if (!bytes) return undefined; // blob-backed, not fetchable here
    return `data:${IMAGE_MIME[kind]};base64,${bytesToBase64(bytes)}`;
  }
  return undefined;
}

export function hasBlobPayload(clip: Clip): boolean {
  return clip.payloads.some((p) => p.body === "Blob");
}

/** A single, generously permissive URL check — this only needs to catch the
 * common "I copied a link" case for the Links filter, not validate URLs. */
const LOOKS_LIKE_URL = /^(https?:\/\/|www\.)\S+$/i;

export function looksLikeLink(clip: Clip): boolean {
  if (clip.kind !== "text") return false;
  const text = getClipText(clip) ?? clip.preview;
  return LOOKS_LIKE_URL.test(text.trim());
}

/** Maps the backend's `ClipKind` (plus a client-side "is this a URL" check,
 * since the daemon has no `link` kind) onto the four tabs the UI exposes:
 * all / text / image / files / link. `html` and `rtf` clips are grouped
 * under "text" — to the person using Clipse they are just rich text. */
export function matchesTypeFilter(clip: Clip, filter: ClipTypeFilter): boolean {
  switch (filter) {
    case "all":
      return true;
    case "image":
      return clip.kind === "image";
    case "files":
      return clip.kind === "files";
    case "link":
      return looksLikeLink(clip);
    case "text":
      return clip.kind === "text" || clip.kind === "html" || clip.kind === "rtf";
    default:
      return true;
  }
}

/** The server-side `HistoryQuery.kind` value that exactly matches a filter,
 * or `null` when the filter has no 1:1 backend kind (text/link/all all
 * cover more than one `ClipKind`, or need a client-side content check) and
 * must be narrowed client-side instead after an unfiltered fetch. */
export function serverKindForFilter(filter: ClipTypeFilter): ClipKind | null {
  if (filter === "image") return "image";
  if (filter === "files") return "files";
  return null;
}

export function humanBytes(bytes: number): string {
  const KB = 1024;
  const MB = KB * 1024;
  const GB = MB * 1024;
  if (bytes >= GB) return `${(bytes / GB).toFixed(1)} GB`;
  if (bytes >= MB) return `${(bytes / MB).toFixed(1)} MB`;
  if (bytes >= KB) return `${Math.round(bytes / KB)} KB`;
  return `${bytes} B`;
}
