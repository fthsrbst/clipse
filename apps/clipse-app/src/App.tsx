import { useState } from "react";

import { HistoryWindow } from "./pages/history-window";
import { PopupWindow } from "./pages/popup-window";
import { Onboarding, hasSeenOnboarding } from "./components/onboarding";
import { currentWindowLabel } from "./lib/tauri-client";
import { useSettings } from "./hooks/use-settings";
import { LABEL as POPUP_LABEL } from "./lib/window-labels";

/**
 * A single Vite app backs both Tauri windows declared in
 * `src-tauri/tauri.conf.json` ("main" and "popup") — this picks the surface
 * to render based on which window this JS context is actually running in.
 * Settings lives inside the History window as an internal view (there is no
 * separate OS-level window for it).
 *
 * The introduction is only ever the main window's problem. The popup is
 * summoned by a hotkey in the middle of somebody's work; interrupting that
 * with a welcome sequence would be the opposite of the point.
 */
export default function App() {
  const label = currentWindowLabel();
  if (label === POPUP_LABEL) {
    return <PopupWindow />;
  }
  return <MainWindow />;
}

function MainWindow() {
  // Read once on mount: flipping to the history the instant the flag is
  // written would yank the last step out from under the button that wrote it.
  const [introducing, setIntroducing] = useState(() => !hasSeenOnboarding());
  const { settings } = useSettings();

  if (introducing) {
    return (
      <Onboarding
        hotkey={settings?.hotkey ?? "Ctrl+Shift+V"}
        onDone={() => setIntroducing(false)}
      />
    );
  }
  return <HistoryWindow />;
}
