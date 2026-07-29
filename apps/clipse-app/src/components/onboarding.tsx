import { useCallback, useEffect, useLayoutEffect, useRef, useState } from "react";

import { EclipseCanvas } from "./eclipse-canvas";
import { formatHotkey } from "../lib/format-hotkey";
import { EASE, duration, gsap, stagger } from "../lib/motion";
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

/** Where the text block sits. Each step lands somewhere different, so the eye
 * has to travel and the sequence never settles into a template. */
type Anchor = "bottom-left" | "top-right" | "mid-left" | "bottom-right";

interface Step {
  /** Where the moon is while this step is on screen. */
  phase: number;
  anchor: Anchor;
  index: string;
  kicker: string;
  title: string;
  body: string;
  aside?: string;
}

/* The eclipse carries the argument, not decoration: the disc goes dark on the
 * screen about secrets never being written down, and comes back out the other
 * side when it is time to actually use the thing. */
const STEPS: Step[] = [
  {
    phase: 0.03,
    anchor: "bottom-left",
    index: "01",
    kicker: "What this is",
    title: "Everything you copy, kept.",
    body: "Text, links, images, files — remembered for as long as you want them. No limit, no cloud, no account. The history is a file on this machine and nowhere else.",
    aside: "Nothing is thrown away to make room for something newer.",
  },
  {
    phase: 0.5,
    anchor: "top-right",
    index: "02",
    kicker: "What it refuses to keep",
    title: "Some things are never written down.",
    body: "Copy a password out of your password manager and Clipse does not store it. Not hidden, not encrypted — never written. Same for API keys, card numbers, and anything an app marks as sensitive.",
    aside: "Enforced in the capture path, before the history exists. Not a setting you can get wrong.",
  },
  {
    phase: 0.5,
    anchor: "mid-left",
    index: "03",
    kicker: "Your other machines",
    title: "Your devices. Nobody else's.",
    body: "Pairing takes a one-time six-digit code you confirm on both screens. Devices that never exchanged one cannot see your clipboard, cannot ask for it, and cannot join by being on the same network.",
    aside: "Sync runs directly between your machines. There is no server in the middle.",
  },
  {
    phase: 0.93,
    anchor: "bottom-right",
    index: "04",
    kicker: "Using it",
    title: "One shortcut. Anywhere.",
    body: "Press the hotkey in any application, pick a clip with the arrow keys, hit Enter. It pastes straight into whatever you were doing.",
    aside: "Clipse lives in the tray. Close the window and it keeps working.",
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
  const copyRef = useRef<HTMLElement | null>(null);
  const chromeRef = useRef<HTMLDivElement | null>(null);

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

  // The frame arrives once; the copy re-animates on every step. Splitting them
  // means the masthead and controls do not flicker each time someone advances.
  useLayoutEffect(() => {
    if (!chromeRef.current) return;
    const targets = chromeRef.current.querySelectorAll("[data-chrome]");
    gsap.fromTo(
      targets,
      { opacity: 0 },
      { opacity: 1, duration: duration("slow"), ease: EASE.out, stagger: stagger(2) },
    );
  }, []);

  useLayoutEffect(() => {
    const block = copyRef.current;
    if (!block) return;

    const lines = block.querySelectorAll("[data-line]");
    const ctx = gsap.context(() => {
      gsap
        .timeline()
        .fromTo(
          block,
          { opacity: 0 },
          { opacity: 1, duration: duration("fast"), ease: EASE.out },
        )
        .fromTo(
          lines,
          { opacity: 0, yPercent: 40, filter: "blur(6px)" },
          {
            opacity: 1,
            yPercent: 0,
            filter: "blur(0px)",
            duration: duration("slow"),
            ease: EASE.out,
            stagger: stagger(1.6),
            clearProps: "filter,transform",
          },
          "<",
        );
    }, block);

    return () => ctx.revert();
  }, [step]);

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
    <div className={styles.root} ref={chromeRef}>
      {/* Full bleed and behind everything: the artwork is the room, not a panel
       * in it, which is the whole difference between this and a slide.
       *
       * It also leans away from wherever the text has landed. That keeps the
       * composition asymmetric — the point — while giving the words a quiet
       * corner to be read in, which a centred field cannot do. */}
      <div className={styles.field} data-away-from={current.anchor} aria-hidden="true">
        <EclipseCanvas phase={current.phase} />
      </div>

      <div className={styles.vignette} aria-hidden="true" />

      <header className={styles.masthead} data-chrome>
        <span className={styles.wordmark}>Clipse</span>
        <button type="button" className={styles.skip} onClick={finish}>
          Skip
        </button>
      </header>

      {/* The step numeral runs up the left edge, rotated, like a spine. */}
      <div className={styles.spine} data-chrome aria-hidden="true">
        <span className={styles.spineIndex} data-numeric>
          {current.index}
        </span>
        <span className={styles.spineRule} />
        <span className={styles.spineTotal} data-numeric>
          {String(STEPS.length).padStart(2, "0")}
        </span>
      </div>

      <article className={styles.copy} data-anchor={current.anchor} ref={copyRef}>
        <p className={styles.kicker} data-line>
          {current.kicker}
        </p>
        <h1 className={styles.title} data-line>
          {current.title}
        </h1>
        <p className={styles.body} data-line>
          {current.body}
        </p>
        {current.aside && (
          <p className={styles.aside} data-line>
            {current.aside}
          </p>
        )}
        {last && (
          <p className={styles.hotkey} data-line>
            <kbd className={styles.kbd}>{formatHotkey(hotkey)}</kbd>
          </p>
        )}
      </article>

      <footer className={styles.footer} data-chrome>
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
  );
}
