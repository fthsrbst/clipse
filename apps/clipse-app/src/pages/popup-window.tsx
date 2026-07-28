import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { SearchBox } from "../components/search-box";
import { TypeFilterTabs } from "../components/type-filter-tabs";
import { ClipList } from "../components/clip-list";
import { EmptyState } from "../components/empty-state";
import { DaemonOfflineState } from "../components/daemon-offline-state";
import { fuzzyFilter } from "../lib/fuzzy";
import { matchesTypeFilter } from "../lib/clip-content";
import {
  type PopupKeyAction,
  type PopupKeyEffect,
  type PopupKeyState,
  initialPopupKeyState,
  popupKeyReducer,
} from "../lib/popup-reducer";
import { usePopupClips } from "../hooks/use-popup-clips";
import { usePopupMotion } from "../hooks/use-popup-motion";
import { api } from "../lib/tauri-client";
import type { Clip } from "../types/ipc";
import styles from "./popup-window.module.css";

/** `dismiss` owns both the exit animation and the actual hide, so every path
 * that closes the popup goes through it rather than calling `hidePopup`. */
function performEffect(
  effect: PopupKeyEffect,
  results: Clip[],
  dismiss: (before?: () => Promise<void>) => Promise<void>,
) {
  if (effect.type === "paste") {
    const target = results[effect.index];
    if (!target) return;
    void dismiss(() => api.paste(target.id));
  } else if (effect.type === "close") {
    void dismiss();
  }
}

const ROW_HEIGHT = 52;

export function PopupWindow() {
  const { root, dismiss } = usePopupMotion();
  const { clips, loading, offline, refresh } = usePopupClips();
  const [query, setQuery] = useState("");
  const [keyState, setKeyState] = useState<PopupKeyState>(initialPopupKeyState());
  const inputRef = useRef<HTMLInputElement>(null);

  const byFilter = useMemo(
    () => clips.filter((c) => matchesTypeFilter(c, keyState.filter)),
    [clips, keyState.filter],
  );

  const results: Clip[] = useMemo(() => {
    if (query.trim().length === 0) return byFilter;
    return fuzzyFilter(query, byFilter, (c) => c.preview).map((hit) => hit.item);
  }, [byFilter, query]);

  // The result count can change from either typing or a live clip event —
  // keep the reducer's notion of the list size (and thus selection clamping
  // and wraparound) in sync with it.
  useEffect(() => {
    setKeyState((s) => popupKeyReducer(s, { type: "SetItemCount", count: results.length }).state);
  }, [results.length]);

  // Deliberately *not* computed inside a `setState` functional updater: React
  // (in development, under StrictMode) invokes updater callbacks twice to
  // help surface impure ones, which would fire `paste`/`hide_popup` twice
  // for a single keypress. `keyState` here is already this render's
  // committed value, which is all a single, synchronous keyboard event
  // needs — so the reducer runs once, plainly, against it.
  const dispatch = useCallback(
    (action: PopupKeyAction) => {
      const { state, effect } = popupKeyReducer(keyState, action);
      setKeyState(state);
      performEffect(effect, results, dismiss);
    },
    [keyState, results, dismiss],
  );

  const resetForReopen = useCallback(() => {
    setQuery("");
    setKeyState((s) => ({ ...s, selectedIndex: 0 }));
    refresh();
    // Focus after the show animation would otherwise steal it back.
    requestAnimationFrame(() => inputRef.current?.focus());
  }, [refresh]);

  useEffect(() => {
    inputRef.current?.focus();
    // The popup window is shown/hidden, not remounted, so "reopened" is
    // observed as the webview regaining focus.
    window.addEventListener("focus", resetForReopen);
    return () => window.removeEventListener("focus", resetForReopen);
  }, [resetForReopen]);

  function onKeyDown(e: React.KeyboardEvent) {
    if (e.key === "ArrowDown") {
      e.preventDefault();
      dispatch({ type: "ArrowDown" });
    } else if (e.key === "ArrowUp") {
      e.preventDefault();
      dispatch({ type: "ArrowUp" });
    } else if (e.key === "Tab") {
      e.preventDefault();
      dispatch({ type: "Tab" });
    } else if (e.key === "Enter") {
      e.preventDefault();
      dispatch({ type: "Enter" });
    } else if (e.key === "Escape") {
      e.preventDefault();
      dispatch({ type: "Escape" });
    } else if ((e.ctrlKey || e.metaKey) && /^[1-9]$/.test(e.key)) {
      e.preventDefault();
      dispatch({ type: "JumpTo", index: Number(e.key) - 1 });
    }
  }

  return (
    <div className={styles.card} onKeyDown={onKeyDown} ref={root}>
      <div className={styles.searchRow}>
        <SearchBox ref={inputRef} value={query} onChange={setQuery} placeholder="Type to filter…" autoFocus />
      </div>

      <div className={styles.filterRow}>
        <TypeFilterTabs
          value={keyState.filter}
          onChange={(filter) => setKeyState((s) => ({ ...s, filter, selectedIndex: 0 }))}
          compact
        />
      </div>

      <div className={styles.listWrap}>
        {offline ? (
          <DaemonOfflineState />
        ) : loading && clips.length === 0 ? (
          <EmptyState title="Loading…" animated />
        ) : results.length === 0 ? (
          <EmptyState title={query ? "No matches" : "Nothing copied yet"} />
        ) : (
          <ClipList
            clips={results}
            itemHeight={ROW_HEIGHT}
            compact
            showShortcutBadges
            selectedIndex={keyState.selectedIndex}
            onActivate={(clip) => {
              void api
                .paste(clip.id)
                .catch(() => {})
                .finally(() => void api.hidePopup());
            }}
          />
        )}
      </div>

      <div className={styles.hints}>
        <span>↑↓ navigate</span>
        <span>Enter paste</span>
        <span>⌘1–9 quick paste</span>
        <span>Tab filter</span>
        <span>Esc close</span>
      </div>
    </div>
  );
}
