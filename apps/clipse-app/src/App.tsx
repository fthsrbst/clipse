import { HistoryWindow } from "./pages/history-window";
import { PopupWindow } from "./pages/popup-window";
import { currentWindowLabel } from "./lib/tauri-client";
import { LABEL as POPUP_LABEL } from "./lib/window-labels";

/**
 * A single Vite app backs both Tauri windows declared in
 * `src-tauri/tauri.conf.json` ("main" and "popup") — this picks the surface
 * to render based on which window this JS context is actually running in.
 * Settings lives inside the History window as an internal view (there is no
 * separate OS-level window for it).
 */
export default function App() {
  const label = currentWindowLabel();
  return label === POPUP_LABEL ? <PopupWindow /> : <HistoryWindow />;
}
