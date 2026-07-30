import { renderHook, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { useClipPayload } from "./use-clip-payload";
import type { Clip } from "../types/ipc";

const getPayload = vi.fn();
vi.mock("../lib/tauri-client", () => ({
  api: { getPayload: (...args: unknown[]) => getPayload(...args) },
}));

/** An image clip whose single PNG payload is inline or blob-backed. */
function imageClip(body: "Blob" | { Inline: number[] }, size: number): Clip {
  return {
    id: "c1",
    hash: "a".repeat(64),
    kind: "image",
    payloads: [{ format: "Png", digest: "b".repeat(64), size, body }],
    preview: "screenshot",
    source: { device: "d1", device_label: "This machine", app: "Snipping Tool" },
    hlc: { wall_ms: 1, counter: 0, device: "d1" },
    created_at_ms: 1,
    pinned: false,
    deleted: false,
  };
}

describe("useClipPayload", () => {
  beforeEach(() => getPayload.mockReset());

  it("uses inline bytes without asking the daemon", async () => {
    const { result } = renderHook(() => useClipPayload(imageClip({ Inline: [104, 105] }, 2)));

    await waitFor(() => expect(result.current.imageUrl).toContain("data:image/png;base64,"));
    expect(getPayload).not.toHaveBeenCalled();
  });

  it("asks the daemon for a blob-backed payload", async () => {
    getPayload.mockResolvedValue("aGk=");
    const { result } = renderHook(() => useClipPayload(imageClip("Blob", 500_000)));

    await waitFor(() => expect(result.current.imageUrl).toBe("data:image/png;base64,aGk="));
    expect(getPayload).toHaveBeenCalledWith("c1", "Png");
    expect(result.current.tooLarge).toBe(false);
  });

  it("reports an over-cap payload as too large rather than as an error", async () => {
    // The daemon answers None past MAX_PAYLOAD_BYTES; the panel shows a size.
    getPayload.mockResolvedValue(null);
    const { result } = renderHook(() => useClipPayload(imageClip("Blob", 40_000_000)));

    await waitFor(() => expect(result.current.loading).toBe(false));
    expect(result.current.imageUrl).toBeNull();
    expect(result.current.tooLarge).toBe(true);
  });

  it("declines a payload over the caller's own cap without asking", async () => {
    // A list of rows must not each pull a screenshot over IPC. The size is on
    // the clip already, so the request is skipped rather than made and thrown
    // away.
    const { result } = renderHook(() =>
      useClipPayload(imageClip("Blob", 9_000_000), { maxBytes: 4 * 1024 * 1024 }),
    );

    await waitFor(() => expect(result.current.loading).toBe(false));
    expect(getPayload).not.toHaveBeenCalled();
    expect(result.current.tooLarge).toBe(true);
    expect(result.current.imageUrl).toBeNull();
  });

  it("still fetches a payload under the caller's cap", async () => {
    getPayload.mockResolvedValue("aGk=");
    const { result } = renderHook(() =>
      useClipPayload(imageClip("Blob", 500_000), { maxBytes: 4 * 1024 * 1024 }),
    );

    await waitFor(() => expect(result.current.imageUrl).toBe("data:image/png;base64,aGk="));
  });

  it("asks for nothing at all when there is no clip to show", () => {
    const { result } = renderHook(() => useClipPayload(null));

    expect(result.current).toEqual({ imageUrl: null, loading: false, tooLarge: false });
    expect(getPayload).not.toHaveBeenCalled();
  });
});
