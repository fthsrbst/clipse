import { getCurrentWindow } from "@tauri-apps/api/window";

import styles from "./window-controls.module.css";

/**
 * Minimise, maximise, close — three mono characters, with no bar around them.
 *
 * That absence is the design. The native title bar was removed because on
 * Windows it sat as a second header above the masthead; drawing our own would
 * be the same problem in our own colours. These sit on the masthead's baseline,
 * and the masthead is the frame.
 */
export function WindowControls() {
  const win = getCurrentWindow();

  return (
    <div className={styles.controls}>
      <button
        type="button"
        className={styles.control}
        aria-label="Minimise"
        onClick={() => void win.minimize()}
      >
        –
      </button>
      <button
        type="button"
        className={styles.control}
        aria-label="Maximise"
        onClick={() => void win.toggleMaximize()}
      >
        ▢
      </button>
      <button
        type="button"
        className={`${styles.control} ${styles.close}`}
        aria-label="Close"
        onClick={() => void win.close()}
      >
        ✕
      </button>
    </div>
  );
}
