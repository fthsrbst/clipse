import { describe, expect, it } from "vitest";

import { formatHotkey } from "./format-hotkey";

// The platform is passed in rather than sniffed so both branches are testable;
// production calls it with the sniffed value.
describe("formatHotkey", () => {
  it("spells modifiers out away from macOS", () => {
    expect(formatHotkey("CmdOrCtrl+Shift+V", false)).toBe("Ctrl + Shift + V");
  });

  it("uses the glyphs, set solid, on macOS", () => {
    expect(formatHotkey("CmdOrCtrl+Shift+V", true)).toBe("⌘⇧V");
  });

  it("resolves CmdOrCtrl to the platform's actual key", () => {
    expect(formatHotkey("CmdOrCtrl+K", false)).toBe("Ctrl + K");
    expect(formatHotkey("CmdOrCtrl+K", true)).toBe("⌘K");
  });

  it("keeps a literal Ctrl as Ctrl on macOS, where it is a separate key", () => {
    expect(formatHotkey("Ctrl+Shift+V", true)).toBe("⌃⇧V");
  });

  it("upper-cases the key so a lowercase accelerator still reads as a keycap", () => {
    expect(formatHotkey("Alt+v", false)).toBe("Alt + V");
  });

  it("names keys that have no glyph", () => {
    expect(formatHotkey("CmdOrCtrl+Space", false)).toBe("Ctrl + Space");
    expect(formatHotkey("Shift+Escape", false)).toBe("Shift + Esc");
  });

  it("passes unknown parts through rather than dropping them", () => {
    expect(formatHotkey("CmdOrCtrl+F13", false)).toBe("Ctrl + F13");
  });

  it("survives the malformed accelerators a settings field can produce", () => {
    expect(formatHotkey("", false)).toBe("");
    expect(formatHotkey("CmdOrCtrl++V", false)).toBe("Ctrl + V");
    expect(formatHotkey("  Shift + V  ", false)).toBe("Shift + V");
  });
});
