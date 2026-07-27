import { useCallback, useEffect, useState } from "react";
import { api, isNotConnected, onClipAdded, onClipRemoved, onClipUpdated } from "../lib/tauri-client";
import type { Clip } from "../types/ipc";

/** Recent-history buffer size for the popup. The popup filters this buffer
 * entirely client-side (fuzzy match, no round trip per keystroke — see
 * `lib/fuzzy.ts`), so it is deliberately generous rather than paginated;
 * unlike the History window it never needs to reach into years-old clips. */
const POPUP_BUFFER_SIZE = 300;

export interface PopupClips {
  clips: Clip[];
  loading: boolean;
  offline: boolean;
  /** Re-fetch the buffer and clear any stale state — called every time the
   * popup becomes visible again, since Tauri shows/hides the same webview
   * rather than recreating it. */
  refresh: () => void;
}

export function usePopupClips(): PopupClips {
  const [clips, setClips] = useState<Clip[]>([]);
  const [loading, setLoading] = useState(true);
  const [offline, setOffline] = useState(false);

  const refresh = useCallback(() => {
    setLoading(true);
    api
      .history({ limit: POPUP_BUFFER_SIZE, offset: 0, kind: null, pinned_only: false })
      .then((page) => {
        setClips(page.filter((c) => !c.deleted));
        setOffline(false);
      })
      .catch((err) => {
        if (isNotConnected(err)) setOffline(true);
      })
      .finally(() => setLoading(false));
  }, []);

  useEffect(() => {
    refresh();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  useEffect(() => {
    const subs = [
      onClipAdded((clip) =>
        setClips((prev) => [clip, ...prev.filter((c) => c.id !== clip.id)].slice(0, POPUP_BUFFER_SIZE)),
      ),
      onClipUpdated((clip) => setClips((prev) => prev.map((c) => (c.id === clip.id ? clip : c)))),
      onClipRemoved((id) => setClips((prev) => prev.filter((c) => c.id !== id))),
    ];
    return () => subs.forEach((p) => void p.then((unlisten) => unlisten()));
  }, []);

  return { clips, loading, offline, refresh };
}
