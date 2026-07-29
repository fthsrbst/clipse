import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import type { ClipTypeFilter } from "../lib/popup-reducer";
import { matchesTypeFilter, serverKindForFilter } from "../lib/clip-content";
import {
  api,
  isNotConnected,
  onClipAdded,
  onClipRemoved,
  onClipUpdated,
  onConnectionChanged,
} from "../lib/tauri-client";
import type { Clip, HistoryQuery } from "../types/ipc";

const PAGE_SIZE = 150;

/** Retries while the embedded daemon is still starting. Bounded, because past
 * this point the daemon really is not coming and saying so is the honest
 * answer — roughly ten seconds, which covers opening a fresh database on a
 * slow disk. */
const STARTUP_RETRY_MS = 500;
const STARTUP_ATTEMPTS = 20;

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

  /* Coming back from unreachable.
   *
   * The first fetch of a cold launch almost always fails: the daemon starts
   * inside this process and has a database to open, and the window is drawn
   * long before that. Nothing re-triggered the fetch afterwards, so a brand-new
   * install sat on "Clipse isn't running" indefinitely while a perfectly
   * healthy daemon answered every other client on the machine.
   *
   * Both halves are needed. The event covers a daemon that comes up (or comes
   * back) after this mounted; the short retry covers the race where it was
   * already up and the event fired before anyone was listening. */
  useEffect(() => {
    let disposed = false;
    let attempts = 0;
    let timer: ReturnType<typeof setTimeout> | undefined;

    const retry = () => {
      if (disposed || attempts >= STARTUP_ATTEMPTS) return;
      attempts += 1;
      void runFetch(true);
      timer = setTimeout(retry, STARTUP_RETRY_MS);
    };
    timer = setTimeout(retry, STARTUP_RETRY_MS);

    const unlisten = onConnectionChanged((connected) => {
      if (disposed || !connected) return;
      // Connected: stop guessing and read the truth.
      clearTimeout(timer);
      attempts = STARTUP_ATTEMPTS;
      void runFetch(true);
    });

    return () => {
      disposed = true;
      clearTimeout(timer);
      void unlisten.then((fn) => fn());
    };
  }, [runFetch]);

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
