import { WifiOffIcon } from "./icons";
import { AsciiLogo } from "./ascii-logo";
import styles from "./daemon-offline-state.module.css";

export interface DaemonOfflineStateProps {
  onRetry?: () => void;
}

/** Shown instead of the list whenever `clipsed` is unreachable. Deliberately
 * not styled like an error page — a paused eclipse, not a broken one — since
 * this is an expected, common state (the daemon simply hasn't started yet,
 * or was closed) rather than a fault in Clipse itself. */
export function DaemonOfflineState({ onRetry }: DaemonOfflineStateProps) {
  return (
    <div className={styles.wrap}>
      <div className={styles.markWrap}>
        <AsciiLogo variant="mark" cell={7} className={styles.mark} />
        <span className={styles.badge}>
          <WifiOffIcon size={12} />
        </span>
      </div>
      <p className={styles.title}>Clipse isn't running</p>
      <p className={styles.description}>
        The background service that watches your clipboard isn't reachable right now. History and
        sync will pick back up as soon as it's running.
      </p>
      {onRetry && (
        <button type="button" className={styles.retry} onClick={onRetry}>
          Try again
        </button>
      )}
    </div>
  );
}
