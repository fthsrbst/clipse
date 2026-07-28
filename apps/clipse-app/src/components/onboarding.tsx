import { useCallback, useEffect, useState } from "react";

import { EclipseCanvas } from "./eclipse-canvas";
import { formatHotkey } from "../lib/format-hotkey";
import styles from "./onboarding.module.css";

/**
 * Where the first run is decided.
 *
 * Kept in the webview rather than in daemon settings on purpose: whether
 * *this* person has read the introduction is a property of the window, not of
 * the clipboard, and putting it in `Settings` would sync "I have seen the
 * onboarding" to every paired device.
 */
const SEEN_KEY = "clipse.onboarding.seen.v1";

export function hasSeenOnboarding(): boolean {
  try {
    return window.localStorage.getItem(SEEN_KEY) === "yes";
  } catch {
    // Private modes and locked-down webviews can throw on access. Showing the
    // introduction again is a far better failure than crashing on launch.
    return false;
  }
}

function rememberSeen() {
  try {
    window.localStorage.setItem(SEEN_KEY, "yes");
  } catch {
    /* Nothing to do; they will see it once more next time. */
  }
}

interface Step {
  /** Where the moon is while this step is on screen. */
  phase: number;
  index: string;
  kicker: string;
  title: string;
  body: string;
  aside?: string;
}

/* The eclipse is the argument, not decoration: the disc goes dark exactly
 * where the copy says nothing leaves the machine, and comes back out the other
 * side when it is time to actually use the thing. */
const STEPS: Step[] = [
  {
    phase: 0.03,
    index: "01",
    kicker: "What this is",
    title: "Everything you copy, kept.",
    body: "Clipse remembers your clipboard — text, links, images, files — and keeps it as long as you want it. No limit, no cloud, no account. The history lives in a file on this machine and nowhere else.",
    aside: "Search it any time. Nothing is thrown away to make room for something newer.",
  },
  {
    phase: 0.5,
    index: "02",
    kicker: "What it refuses to keep",
    title: "Some things are never written down.",
    body: "Copy a password out of your password manager and Clipse does not store it. Not hidden, not encrypted — never written. The same goes for API keys, card numbers and anything an app marks as sensitive.",
    aside: "This is enforced in the capture path, before the history exists. It is not a setting you can get wrong.",
  },
  {
    phase: 0.5,
    index: "03",
    kicker: "Your other machines",
    title: "Your devices, and nobody else's.",
    body: "Pairing takes a one-time six-digit code that you confirm on both screens. Devices that never exchanged one cannot see your clipboard, cannot ask for it, and cannot join by being on the same network.",
    aside: "Sync goes directly between your machines over the local network or your tailnet. There is no server in the middle.",
  },
  {
    phase: 0.93,
    index: "04",
    kicker: "Using it",
    title: "One shortcut, anywhere.",
    body: "Press the hotkey in any application to bring up your recent clips, pick one with the arrow keys, and press Enter to paste it straight into whatever you were doing.",
    aside: "Clipse stays out of the way in the tray. Close this window and it keeps working.",
  },
];

interface Props {
  onDone: () => void;
  /** Shown on the last step so the introduction ends on the real thing. */
  hotkey: string;
}

export function Onboarding({ onDone, hotkey }: Props) {
  const [step, setStep] = useState(0);
  const last = step === STEPS.length - 1;

  const finish = useCallback(() => {
    rememberSeen();
    onDone();
  }, [onDone]);

  const next = useCallback(() => {
    if (last) {
      finish();
    } else {
      setStep((s) => s + 1);
    }
  }, [last, finish]);

  const back = useCallback(() => setStep((s) => Math.max(0, s - 1)), []);

  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (event.key === "ArrowRight" || event.key === "Enter") {
        event.preventDefault();
        next();
      } else if (event.key === "ArrowLeft") {
        event.preventDefault();
        back();
      } else if (event.key === "Escape") {
        event.preventDefault();
        finish();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [next, back, finish]);

  const current = STEPS[step];

  return (
    <div className={styles.root}>
      <figure className={styles.stage}>
        <EclipseCanvas phase={current.phase} label="An eclipse, drawn in characters" />
      </figure>

      <div className={styles.column}>
        <header className={styles.masthead}>
          <span className={styles.wordmark}>Clipse</span>
          <button type="button" className={styles.skip} onClick={finish}>
            Skip
          </button>
        </header>

        {/* Keyed on the step so React remounts it and the entrance animation
         * replays; without the key the text would swap in place and the whole
         * sequence would feel like a slideshow of one slide. */}
        <article className={styles.copy} key={step}>
          <p className={styles.kicker}>
            <span className={styles.index} data-numeric>
              {current.index}
            </span>
            {current.kicker}
          </p>
          <h1 className={styles.title}>{current.title}</h1>
          <p className={styles.body}>{current.body}</p>
          {current.aside && <p className={styles.aside}>{current.aside}</p>}
          {last && (
            <p className={styles.hotkey}>
              <kbd className={styles.kbd}>{formatHotkey(hotkey)}</kbd>
            </p>
          )}
        </article>

        <footer className={styles.footer}>
          <ol className={styles.ticks} aria-label={`Step ${step + 1} of ${STEPS.length}`}>
            {STEPS.map((s, i) => (
              <li
                key={s.index}
                className={styles.tick}
                data-state={i === step ? "current" : i < step ? "past" : "future"}
              />
            ))}
          </ol>

          <div className={styles.actions}>
            {step > 0 && (
              <button type="button" className={styles.back} onClick={back}>
                Back
              </button>
            )}
            <button type="button" className={styles.next} onClick={next}>
              {last ? "Start" : "Next"}
            </button>
          </div>
        </footer>
      </div>
    </div>
  );
}
