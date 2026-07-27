import { useState } from "react";
import styles from "./hotkey-input.module.css";

export interface HotkeyInputProps {
  value: string;
  onChange: (accelerator: string) => void;
}

const MODIFIER_KEYS = new Set(["Control", "Shift", "Alt", "Meta", "OS"]);

/** Formats a KeyboardEvent as a Tauri accelerator string, e.g.
 * `CmdOrCtrl+Shift+V`. See `src-tauri/src/hotkey.rs` — this is registered
 * with `tauri-plugin-global-shortcut` verbatim. */
function acceleratorFromEvent(e: React.KeyboardEvent): string | null {
  if (MODIFIER_KEYS.has(e.key)) return null;

  const parts: string[] = [];
  if (e.ctrlKey || e.metaKey) parts.push("CmdOrCtrl");
  if (e.shiftKey) parts.push("Shift");
  if (e.altKey) parts.push("Alt");

  const key = e.key.length === 1 ? e.key.toUpperCase() : e.key;
  parts.push(key);
  return parts.join("+");
}

/** Click to record, then press the desired combination — mirrors how every
 * OS-level shortcut picker works, and sidesteps the awkwardness of typing an
 * accelerator string by hand. */
export function HotkeyInput({ value, onChange }: HotkeyInputProps) {
  const [recording, setRecording] = useState(false);

  function onKeyDown(e: React.KeyboardEvent<HTMLButtonElement>) {
    if (!recording) return;
    e.preventDefault();
    if (e.key === "Escape") {
      setRecording(false);
      return;
    }
    const accelerator = acceleratorFromEvent(e);
    if (accelerator) {
      onChange(accelerator);
      setRecording(false);
    }
  }

  return (
    <button
      type="button"
      className={recording ? `${styles.field} ${styles.recording}` : styles.field}
      onClick={() => setRecording(true)}
      onBlur={() => setRecording(false)}
      onKeyDown={onKeyDown}
    >
      {recording ? "Press a key combination…" : value}
    </button>
  );
}
