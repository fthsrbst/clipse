import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { Spine } from "../components/spine";
import { SearchBox } from "../components/search-box";
import { TypeFilterTabs } from "../components/type-filter-tabs";
import { ClipList } from "../components/clip-list";
import { EmptyState } from "../components/empty-state";
import { DaemonOfflineState } from "../components/daemon-offline-state";
import { CaptureModeBanner } from "../components/capture-mode-banner";
import { ResizeHandles } from "../components/resize-handles";
import { WindowControls } from "../components/window-controls";
import { PinFilledIcon, PinIcon } from "../components/icons";
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

  // Escape leaves settings. There is no back button to draw now that the spine
  // holds the navigation, so the key has to carry it.
  useEffect(() => {
    if (view !== "settings") return;
    const onKey = (event: KeyboardEvent) => {
      if (event.key === "Escape") setView("history");
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [view]);

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

  return (
    <div className={styles.window} ref={root}>
      <ResizeHandles />

      {/* Mounted once and kept across both views: settings is a second view of
       * this window, not a screen you leave for — which is what removes the
       * need for a back button. */}
      <Spine
        clipCount={history.clips.length}
        secretsRefused={status?.secrets_refused ?? 0}
        paused={status?.paused ?? false}
        loadingMore={history.loadingMore}
        peersOnline={status?.peers_online ?? 0}
        peersTotal={status?.peers_total ?? 0}
        settingsActive={view === "settings"}
        onToggleSettings={() => setView(view === "settings" ? "history" : "settings")}
        countRef={countRef}
      />

      <div className={styles.main}>
        {view === "settings" ? (
          <SettingsView status={status} />
        ) : (
          <>
      {/* No title bar and no toolbar. One row carries search, the filters and
       * the window controls, and the space around it is the drag region — so
       * the frame costs no vertical space of its own. */}
      <div className={styles.top} data-tauri-drag-region data-enter>
        <SearchBox
          value={history.searchText}
          onChange={history.setSearchText}
          placeholder="Search everything you've copied"
        />
        <TypeFilterTabs value={history.typeFilter} onChange={history.setTypeFilter} />
        <button
          type="button"
          className={history.pinnedOnly ? `${styles.pinToggle} ${styles.active}` : styles.pinToggle}
          aria-label="Show pinned only"
          aria-pressed={history.pinnedOnly}
          onClick={() => history.setPinnedOnly(!history.pinnedOnly)}
        >
          {history.pinnedOnly ? <PinFilledIcon size={15} /> : <PinIcon size={15} />}
        </button>
        <WindowControls />
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
          </>
        )}
      </div>
    </div>
  );
}
