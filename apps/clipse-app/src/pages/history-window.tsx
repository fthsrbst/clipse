import { useState } from "react";
import { EclipseMark } from "../components/eclipse-mark";
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
import { SettingsView } from "./settings-view";
import type { Clip } from "../types/ipc";
import styles from "./history-window.module.css";

const ROW_HEIGHT = 56;

export function HistoryWindow() {
  const [view, setView] = useState<"history" | "settings">("history");
  const history = useClipHistory();
  const { status } = useDaemonConnection();

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
    <div className={styles.window}>
      <header className={styles.header}>
        <div className={styles.brand}>
          <EclipseMark size={22} />
          <span className={styles.title}>Clipse</span>
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

      <div className={styles.toolbar}>
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

      <footer className={styles.footer}>
        <span>
          {status ? `${status.clip_count} clip${status.clip_count === 1 ? "" : "s"}` : "…"}
        </span>
        {status?.paused && <span className={styles.paused}>Paused</span>}
        {history.loadingMore && <span>Loading more…</span>}
      </footer>
    </div>
  );
}
