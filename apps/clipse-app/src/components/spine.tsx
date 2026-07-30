import { AsciiLogo } from "./ascii-logo";
import { SettingsIcon } from "./icons";
import styles from "./spine.module.css";

export interface SpineProps {
  clipCount: number;
  /** Captures dropped for looking like a secret, since the daemon started. */
  secretsRefused: number;
  paused: boolean;
  loadingMore: boolean;
  peersOnline: number;
  peersTotal: number;
  settingsActive: boolean;
  onToggleSettings: () => void;
  /** The count element, so the window can animate it when it changes. */
  countRef?: React.Ref<HTMLSpanElement>;
}

/**
 * The left rail, and the window's entire chrome.
 *
 * It replaces a header, a footer, and the right-hand end of a toolbar. The
 * onboarding sets its step numeral on a rotated spine; this is that spine made
 * permanent, and the composition it forces — everything structural down one
 * narrow edge, the content given the whole rest of the window — is what lets a
 * frameless window have no title bar to reconcile.
 *
 * Its empty middle is the drag region: space the composition wanted anyway, so
 * the frame costs no vertical room.
 */
export function Spine({
  clipCount,
  secretsRefused,
  paused,
  loadingMore,
  peersOnline,
  peersTotal,
  settingsActive,
  onToggleSettings,
  countRef,
}: SpineProps) {
  return (
    <aside className={styles.spine}>
      <AsciiLogo variant="mark" cell={7} className={styles.mark} />
      <span className={styles.wordmark} aria-hidden="true">
        CLIPSE
      </span>

      <div className={styles.meter}>
        <span className={styles.count} data-numeric ref={countRef}>
          {clipCount}
        </span>
        <span className={styles.label}>{clipCount === 1 ? "clip" : "clips"}</span>
      </div>

      {/* The promise, made visible.
       *
       * A count and nothing else, because the daemon does not record what it
       * refused — there is nothing else it could show. Set in normal ink
       * however high it climbs: the accent belongs to the moment of an
       * increment and to the hovered close control, and two standing reds
       * would make neither mean anything. Zero is shown rather than hidden,
       * because "nothing has been refused" is also information. */}
      <div className={styles.refused} title="Captures refused for looking like a secret">
        <span className={styles.refusedCount} data-numeric>
          {secretsRefused}
        </span>
        <span className={styles.label}>refused</span>
      </div>

      <div className={styles.state}>
        {paused && <span className={styles.paused}>paused</span>}
        {loadingMore && <span className={styles.loading}>loading</span>}
      </div>

      {/* Everything the spine has to say is said above; this is the rest of the
       * rail, and it is the drag region. Below the content rather than through
       * the middle of it — a gap between the identity and its own numbers
       * reads as abandoned space rather than as composition. */}
      <div className={styles.drag} data-tauri-drag-region />

      <div className={styles.foot}>
        {peersTotal > 0 && (
          <span
            className={styles.peers}
            title={`${peersOnline} of ${peersTotal} paired devices online`}
          >
            {Array.from({ length: peersTotal }, (_, i) => (
              <span key={i} className={i < peersOnline ? styles.dotOn : styles.dotOff} />
            ))}
          </span>
        )}
        <button
          type="button"
          className={settingsActive ? `${styles.settings} ${styles.on}` : styles.settings}
          aria-label="Settings"
          aria-pressed={settingsActive}
          onClick={onToggleSettings}
        >
          <SettingsIcon size={15} />
        </button>
      </div>
    </aside>
  );
}
