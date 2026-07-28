/**
 * Turn a Tauri accelerator into something a person would recognise.
 *
 * Settings store `CmdOrCtrl+Shift+V`, which is the right thing to store — one
 * string that means the native chord on every platform. It is the wrong thing
 * to *show*: nobody has a key called CmdOrCtrl, and printing the stored form is
 * how software admits it was written for the machine rather than the reader.
 */

const MAC_SYMBOLS: Record<string, string> = {
  cmdorctrl: "⌘",
  cmd: "⌘",
  command: "⌘",
  ctrl: "⌃",
  control: "⌃",
  alt: "⌥",
  option: "⌥",
  shift: "⇧",
  enter: "↩",
  return: "↩",
  escape: "⎋",
  space: "Space",
};

const OTHER_NAMES: Record<string, string> = {
  cmdorctrl: "Ctrl",
  cmd: "Ctrl",
  command: "Ctrl",
  ctrl: "Ctrl",
  control: "Ctrl",
  alt: "Alt",
  option: "Alt",
  shift: "Shift",
  enter: "Enter",
  return: "Enter",
  escape: "Esc",
  space: "Space",
};

function isMac(): boolean {
  // `navigator.platform` is deprecated but still the only thing available in a
  // webview without asking the Rust side; the userAgent check covers the newer
  // Apple Silicon strings.
  const hint = `${navigator.platform} ${navigator.userAgent}`;
  return /Mac|iPhone|iPad/i.test(hint);
}

export function formatHotkey(accelerator: string, mac = isMac()): string {
  const table = mac ? MAC_SYMBOLS : OTHER_NAMES;
  const parts = accelerator
    .split("+")
    .map((part) => part.trim())
    .filter(Boolean)
    .map((part) => table[part.toLowerCase()] ?? part.toUpperCase());

  // macOS writes chords solid — ⌘⇧V — while everywhere else spells them out.
  return mac ? parts.join("") : parts.join(" + ");
}
