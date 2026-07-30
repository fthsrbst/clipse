import { getCurrentWindow } from "@tauri-apps/api/window";

import { RESIZE_EDGES, resizeDirection } from "../lib/window-frame";
import styles from "./resize-handles.module.css";

/**
 * Eight invisible grips around the window edge.
 *
 * A frameless Win32 window has no resize border, so without these the window is
 * a fixed size and nothing on screen explains why. Kept as its own component
 * because it is frame plumbing and belongs nowhere near the layout it happens
 * to sit inside.
 */
export function ResizeHandles() {
  const win = getCurrentWindow();

  return (
    <>
      {RESIZE_EDGES.map((edge) => (
        <div
          key={edge}
          className={`${styles.handle} ${styles[edge]}`}
          data-edge={edge}
          onMouseDown={(event) => {
            if (event.button !== 0) return;
            event.preventDefault();
            void win.startResizeDragging(resizeDirection(edge) as never);
          }}
        />
      ))}
    </>
  );
}
