import { useEffect, useRef, useState } from "react";

import { findPayload, getClipImageDataUrl, payloadDataUrl } from "../lib/clip-content";
import { api } from "../lib/tauri-client";
import type { Clip } from "../types/ipc";

export interface ClipPayload {
  /** A `data:` URL for the clip's image, once there is one. */
  imageUrl: string | null;
  loading: boolean;
  /** The daemon declined: the payload is past its 24MB preview cap. Not an
   * error — the clip is intact and pastes normally, so the panel shows the
   * size instead of a picture. */
  tooLarge: boolean;
}

const IDLE: ClipPayload = { imageUrl: null, loading: false, tooLarge: false };

/**
 * What a row is willing to pull over IPC for a thumbnail.
 *
 * The daemon's own cap is 24MB, which is the right number for "show me this
 * clip". It is the wrong number for a list: a screenful of rows would each ask
 * for one, and ten 24MB payloads arrive as base64 in a webview that has to hold
 * all of them at once. Past this the row shows the size and the detail panel —
 * which fetches one clip, on purpose — shows the picture.
 */
export const THUMBNAIL_MAX_BYTES = 4 * 1024 * 1024;

export interface ClipPayloadOptions {
  /** Skip the fetch above this payload size. Defaults to no limit of our own,
   * leaving only the daemon's. */
  maxBytes?: number;
}

/**
 * The bytes behind a clip, fetched only when something is actually looking.
 *
 * Inline payloads (under 64KB) already travel with the clip and need no request
 * at all. Anything larger has a `Blob` body carrying no bytes, so it takes
 * `get_payload` — which is the whole reason that request exists, since a
 * screenshot is essentially never under 64KB.
 */
export function useClipPayload(clip: Clip | null, options: ClipPayloadOptions = {}): ClipPayload {
  const [state, setState] = useState<ClipPayload>(IDLE);
  const maxBytes = options.maxBytes ?? Number.POSITIVE_INFINITY;

  // Keyed on the id, not the object.
  //
  // A clip is content-addressed: for a given id the payloads cannot change, so
  // the id is the whole of what this effect depends on. Depending on the object
  // instead means any caller that rebuilds it — a `.map`, a fresh fetch, a
  // parent re-render — restarts the fetch, and a caller that rebuilds it *while
  // rendering* spins forever.
  const latest = useRef(clip);
  latest.current = clip;
  const id = clip?.id ?? null;

  useEffect(() => {
    const clip = latest.current;
    if (!clip || clip.kind !== "image") {
      setState(IDLE);
      return;
    }

    const inline = getClipImageDataUrl(clip);
    if (inline) {
      setState({ imageUrl: inline, loading: false, tooLarge: false });
      return;
    }

    const payload =
      findPayload(clip, "Png") ?? findPayload(clip, "Jpeg") ?? findPayload(clip, "Svg");
    if (!payload) {
      setState(IDLE);
      return;
    }

    // Declined before the request rather than after: the caller already knows
    // the size from the clip, so asking and discarding would move bytes for a
    // picture nobody was going to draw.
    if (payload.size > maxBytes) {
      setState({ imageUrl: null, loading: false, tooLarge: true });
      return;
    }

    // Guards a late response for a clip the reader has already moved off,
    // which would otherwise paint the wrong picture into the panel.
    let live = true;
    setState({ imageUrl: null, loading: true, tooLarge: false });

    api
      .getPayload(clip.id, payload.format)
      .then((base64) => {
        if (!live) return;
        const url = base64 === null ? null : payloadDataUrl(payload.format, base64);
        setState({ imageUrl: url, loading: false, tooLarge: base64 === null });
      })
      .catch(() => {
        if (live) setState(IDLE);
      });

    return () => {
      live = false;
    };
  }, [id, maxBytes]);

  return state;
}
