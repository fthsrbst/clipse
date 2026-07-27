import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import type { ClipTypeFilter } from "../lib/popup-reducer";
import { matchesTypeFilter, serverKindForFilter } from "../lib/clip-content";
import { api, isNotConnected, onClipAdded, onClipRemoved, onClipUpdated } from "../lib/tauri-client";
import type { Clip, HistoryQuery } from "../types/ipc";

const PAGE_SIZE = 150;

export interface ClipHistory {
  clips: Clip[];
  searchText: string;
  setSearchText: (text: string) => void;
  typeFilter: ClipTypeFilter;
  setTypeFilter: (filter: ClipTypeFilter) => void;
  pinnedOnly: boolean;
  setPinnedOnly: (pinnedOnly: boolean) => void;
  loading: boolean;
  loadingMore: boolean;
  hasMore: boolean;
  /** `true` once a fetch has come back with `CommandError.kind ===
   * "not_connected"` — the daemon-not-running state the History window
   * renders instead of an empty list. */
  offline: boolean;
  errorMessage: string | null;
  loadMore: () => void;
  reload: () => void;
  /** Optimistic local mutations, ahead of the daemon's `ClipUpdated` /
   * `ClipRemoved` push confirming them. */
  setPinnedLocally: (id: string, pinned: boolean) => void;
  removeLocally: (id: string) => void;
}

/**
 * Data + pagination for the History window.
 *
 * The `image`/`files` type filters map exactly onto `HistoryQuery.kind` and
 * are pushed to the daemon; `text` and `link` do not (a rich-text clip's
 * `kind` is `html`, and "link" is a client-side notion the protocol has no
 * concept of at all — see `matchesTypeFilter`), so those are narrowed after
 * an unfiltered fetch. Pages keep loading until either the visible (post
 * client-filter) list has grown or the daemon runs out of history.
 */
export function useClipHistory(): ClipHistory {
  const [searchText, setSearchText] = useState("");
  const [typeFilter, setTypeFilter] = useState<ClipTypeFilter>("all");
  const [pinnedOnly, setPinnedOnly] = useState(false);
  const [clips, setClips] = useState<Clip[]>([]);
  const [loading, setLoading] = useState(true);
  const [loadingMore, setLoadingMore] = useState(false);
  const [hasMore, setHasMore] = useState(true);
  const [offline, setOffline] = useState(false);
  const [errorMessage, setErrorMessage] = useState<string | null>(null);

  const offsetRef = useRef(0);
  const requestIdRef = useRef(0);
  const paramsRef = useRef({ searchText, typeFilter, pinnedOnly });
  paramsRef.current = { searchText, typeFilter, pinnedOnly };

  const runFetch = useCallback(async (reset: boolean) => {
    const requestId = ++requestIdRef.current;
    const { searchText, typeFilter, pinnedOnly } = paramsRef.current;
    const offset = reset ? 0 : offsetRef.current;
    const query: HistoryQuery = {
      limit: PAGE_SIZE,
      offset,
      kind: serverKindForFilter(typeFilter),
      pinned_only: pinnedOnly,
    };

    if (reset) setLoading(true);
    else setLoadingMore(true);

    try {
      const trimmed = searchText.trim();
      const page = trimmed.length > 0 ? await api.search(trimmed, query) : await api.history(query);
      if (requestId !== requestIdRef.current) return;

      setOffline(false);
      setErrorMessage(null);
      offsetRef.current = offset + page.length;
      setHasMore(page.length === PAGE_SIZE);
      setClips((prev) => (reset ? page : [...prev, ...page]));
    } catch (err) {
      if (requestId !== requestIdRef.current) return;
      if (isNotConnected(err)) {
        setOffline(true);
      } else {
        setErrorMessage(err instanceof Error ? err.message : String(err));
      }
      if (reset) setClips([]);
      setHasMore(false);
    } finally {
      if (requestId === requestIdRef.current) {
        setLoading(false);
        setLoadingMore(false);
      }
    }
  }, []);

  useEffect(() => {
    void runFetch(true);
    // Intentionally re-runs only when a query-shaping input changes.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [searchText, typeFilter, pinnedOnly, runFetch]);

  const loadMore = useCallback(() => {
    if (loading || loadingMore || !hasMore) return;
    void runFetch(false);
  }, [loading, loadingMore, hasMore, runFetch]);

  useEffect(() => {
    const subs = [
      onClipAdded((clip) =>
        setClips((prev) => (prev.some((c) => c.id === clip.id) ? prev : [clip, ...prev])),
      ),
      onClipUpdated((clip) => setClips((prev) => prev.map((c) => (c.id === clip.id ? clip : c)))),
      onClipRemoved((id) => setClips((prev) => prev.filter((c) => c.id !== id))),
    ];
    return () => subs.forEach((p) => void p.then((unlisten) => unlisten()));
  }, []);

  const setPinnedLocally = useCallback((id: string, pinned: boolean) => {
    setClips((prev) => prev.map((c) => (c.id === id ? { ...c, pinned } : c)));
  }, []);

  const removeLocally = useCallback((id: string) => {
    setClips((prev) => prev.filter((c) => c.id !== id));
  }, []);

  const visibleClips = useMemo(
    () => clips.filter((c) => !c.deleted && matchesTypeFilter(c, typeFilter)),
    [clips, typeFilter],
  );

  return {
    clips: visibleClips,
    searchText,
    setSearchText,
    typeFilter,
    setTypeFilter,
    pinnedOnly,
    setPinnedOnly,
    loading,
    loadingMore,
    hasMore,
    offline,
    errorMessage,
    loadMore,
    reload: () => void runFetch(true),
    setPinnedLocally,
    removeLocally,
  };
}
