import { ShieldIcon } from "./icons";
import type { CaptureMode } from "../types/ipc";
import styles from "./capture-mode-banner.module.css";

/**
 * When the daemon reports `capture_mode: ManualPush`, its `reason` string is
 * pre-written for the user (currently: GNOME Wayland has no background
 * clipboard-monitoring protocol) and must be shown verbatim, framed as a
 * limitation of the desktop environment rather than a Clipse malfunction.
 */
export function CaptureModeBanner({ captureMode }: { captureMode: CaptureMode }) {
  if (captureMode === "Automatic") return null;

  return (
    <div className={styles.banner} role="status">
      <ShieldIcon size={16} className={styles.icon} />
      <div>
        <p className={styles.title}>Manual capture on this desktop</p>
        <p className={styles.reason}>{captureMode.ManualPush.reason}</p>
      </div>
    </div>
  );
}
