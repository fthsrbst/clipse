import { useCallback, useEffect, useRef } from "react";

import { api, onPopupShown } from "../lib/tauri-client";

/**
 * Opening and closing the hotkey popup.
 *
 * The window itself cannot be animated: `show()` and `hide()` are OS calls that
 * take effect on the next compositor frame, and the webview is hidden rather
 * than destroyed, so a CSS mount animation would only ever play once. So the
 * *contents* animate, and closing is ordered — play the exit, then hide the
 * window — because hiding first means nobody ever sees the exit.
 *
 * This uses the Web Animations API rather than CSS classes for one reason:
 * `animation.finished` is a promise, so "hide after the animation" is a real
 * await instead of a setTimeout racing a stylesheet.
 */

const ENTER_MS = 190;
const EXIT_MS = 130;

/* Matches --ease-out and --ease-exit in tokens.css. Duplicated because the Web
 * Animations API takes a string, not a custom property; if those change, change
 * these. */
const EASE_OUT = "cubic-bezier(0.16, 1, 0.3, 1)";
const EASE_EXIT = "cubic-bezier(0.4, 0, 1, 1)";

function reducedMotion(): boolean {
  return window.matchMedia("(prefers-reduced-motion: reduce)").matches;
}

export function usePopupMotion() {
  const root = useRef<HTMLDivElement | null>(null);
  /** Guards against a second dismiss landing mid-exit — Escape twice, or Enter
   * on a row while the click is still settling. */
  const leaving = useRef(false);

  const playEnter = useCallback(() => {
    leaving.current = false;
    const element = root.current;
    if (!element) return;

    element.style.opacity = "";
    if (reducedMotion()) return;

    element.animate(
      [
        { opacity: 0, transform: "translateY(-8px) scale(0.975)" },
        { opacity: 1, transform: "none" },
      ],
      { duration: ENTER_MS, easing: EASE_OUT, fill: "both" },
    );
  }, []);

  /**
   * The window is shown by the global hotkey, on the Rust side, which emits
   * `popup:shown` once it is up and focused.
   *
   * Window focus is watched as well, and deliberately so: `dismiss` leaves the
   * element at `opacity: 0`, so if the event were ever missed the popup would
   * come back invisible — a hotkey that appears to do nothing. Two signals for
   * one job is worth it when the failure is silent.
   */
  useEffect(() => {
    playEnter();
    const stop = onPopupShown(playEnter);
    window.addEventListener("focus", playEnter);
    return () => {
      window.removeEventListener("focus", playEnter);
      void stop.then((off) => off());
    };
  }, [playEnter]);

  /**
   * Dismiss the popup, optionally doing something first.
   *
   * `before` runs while the exit plays rather than before it starts — pasting
   * should feel simultaneous with the popup getting out of the way, not
   * sequential with it.
   */
  const dismiss = useCallback(async (before?: () => Promise<void>) => {
    if (leaving.current) return;
    leaving.current = true;

    const element = root.current;
    const work = before?.().catch(() => {}) ?? Promise.resolve();

    if (element && !reducedMotion()) {
      const exit = element.animate(
        [
          { opacity: 1, transform: "none" },
          { opacity: 0, transform: "translateY(-5px) scale(0.99)" },
        ],
        { duration: EXIT_MS, easing: EASE_EXIT, fill: "both" },
      );
      await Promise.all([work, exit.finished.catch(() => {})]);
    } else {
      await work;
    }

    // Left hidden so the next show starts from nothing rather than flashing the
    // last frame of the exit before `playEnter` takes over.
    if (element) element.style.opacity = "0";
    await api.hidePopup().catch(() => {});
  }, []);

  return { root, dismiss };
}
