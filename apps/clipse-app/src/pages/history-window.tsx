import { useLayoutEffect, useRef, useState } from "react";
import { AsciiMark } from "../components/ascii-mark";
import { SearchBox } from "../components/search-box";
import { TypeFilterTabs } from "../components/type-filter-tabs";
import { ClipList } from "../components/clip-list";
import { EmptyState } from "../components/empty-state";
import { DaemonOfflineState } from "../components/daemon-offline-state";
import { CaptureModeBanner } from "../components/capture-mode-banner";
import { PinFilledIcon, PinIcon, SettingsIcon } from "../components/icons";
import { useClipHistory } from "../hooks/use-clip-history";
import { useDaemonConnection } from "../hooks/use-daemon-connection";
import { api } from "../lib/tauri-client";
import { countTo, enter, gsap } from "../lib/motion";
import { SettingsView } from "./settings-view";
import type { Clip } from "../types/ipc";
import styles from "./history-window.module.css";

const ROW_HEIGHT = 56;

export function HistoryWindow() {
  const [view, setView] = useState<"history" | "settings">("history");
  const history = useClipHistory();
  const { status } = useDaemonConnection();
  const root = useRef<HTMLDivElement | null>(null);
  const countRef = useRef<HTMLSpanElement | null>(null);

  // One orchestrated arrival for the window, then never again — reanimating on
  // every state change would turn a tool someone uses fifty times a day into a
  // performance.
  useLayoutEffect(() => {
    if (!root.current) return;
    const ctx = gsap.context(() => {
      enter("[data-enter]", { each: 1.4 });
    }, root.current);
    return () => ctx.revert();
  }, []);

  // The count is the one number worth animating: it is the answer to "did that
  // copy land", which is the question the window exists to answer.
  useLayoutEffect(() => {
    if (countRef.current) countTo(countRef.current, history.clips.length);
  }, [history.clips.length]);

  async function handleCopy(clip: Clip) {
    try {
      await api.apply(clip.id);
    } catch {
      // Surfaced already via history.offline/errorMessage on the next fetch.
    }
  }

  async function handleTogglePin(clip: Clip) {
    const next = !clip.pinned;
    history.setPinnedLocally(clip.id, next);
    try {
      await api.setPinned(clip.id, next);
    } catch {
      history.setPinnedLocally(clip.id, clip.pinned); // roll back
    }
  }

  async function handleDelete(clip: Clip) {
    history.removeLocally(clip.id);
    try {
      await api.delete(clip.id);
    } catch {
      history.reload();
    }
  }

  if (view === "settings") {
    return <SettingsView onBack={() => setView("history")} status={status} />;
  }

  return (
    <div className={styles.window} ref={root}>
      {/* Deliberately not a centred title bar. The wordmark sits hard against
       * the left edge and the count is set as display type on the right, so the
       * top of the window reads as a masthead rather than a toolbar. */}
      <header className={styles.header} data-enter>
        <div className={styles.brand}>
          <AsciiMark />
          <span className={styles.title}>Clipse</span>
        </div>

        <div className={styles.meter}>
          <span className={styles.count} data-numeric ref={countRef}>
            {history.clips.length}
          </span>
          <span className={styles.countLabel}>
            {history.clips.length === 1 ? "clip" : "clips"}
          </span>
        </div>

        <button
          type="button"
          className={styles.iconButton}
          aria-label="Settings"
          onClick={() => setView("settings")}
        >
          <SettingsIcon size={17} />
        </button>
      </header>

      <div className={styles.toolbar} data-enter>
        <SearchBox value={history.searchText} onChange={history.setSearchText} />
        <TypeFilterTabs value={history.typeFilter} onChange={history.setTypeFilter} />
        <button
          type="button"
          className={history.pinnedOnly ? `${styles.iconButton} ${styles.active}` : styles.iconButton}
          aria-label="Show pinned only"
          aria-pressed={history.pinnedOnly}
          onClick={() => history.setPinnedOnly(!history.pinnedOnly)}
        >
          {history.pinnedOnly ? <PinFilledIcon size={16} /> : <PinIcon size={16} />}
        </button>
      </div>

      {status?.capture_mode && status.capture_mode !== "Automatic" && (
        <div className={styles.banner}>
          <CaptureModeBanner captureMode={status.capture_mode} />
        </div>
      )}

      {!history.offline && history.errorMessage && (
        <div className={styles.banner}>
          <p className={styles.errorBanner}>{history.errorMessage}</p>
        </div>
      )}

      <main className={styles.body}>
        {history.offline ? (
          <DaemonOfflineState onRetry={history.reload} />
        ) : history.loading && history.clips.length === 0 ? (
          <EmptyState title="Loading history…" animated />
        ) : history.clips.length === 0 ? (
          <EmptyState
            title={history.searchText || history.typeFilter !== "all" || history.pinnedOnly ? "No matches" : "Nothing copied yet"}
            description={
              history.searchText
                ? `No clips match "${history.searchText}".`
                : history.typeFilter !== "all" || history.pinnedOnly
                  ? "Nothing in this filter yet — try All."
                  : "Copy something and it will show up here instantly."
            }
          />
        ) : (
          <ClipList
            clips={history.clips}
            itemHeight={ROW_HEIGHT}
            onActivate={(clip) => void handleCopy(clip)}
            onTogglePin={handleTogglePin}
            onCopy={handleCopy}
            onDelete={handleDelete}
            onNearEnd={history.loadMore}
          />
        )}
      </main>

      {/* The count moved to the masthead, where it is set as display type. What
       * is left here is state, and only when there is state worth reporting —
       * an empty status bar is a line of chrome earning nothing. */}
      {(status?.paused || history.loadingMore) && (
        <footer className={styles.footer} data-enter>
          {status?.paused && <span className={styles.paused}>Paused</span>}
          {history.loadingMore && <span>Loading more…</span>}
        </footer>
      )}
    </div>
  );
}
