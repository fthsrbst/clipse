import { useEffect, useLayoutEffect, useRef, useState } from "react";

import { render } from "../lib/eclipse-ascii";
import styles from "./eclipse-canvas.module.css";

interface Props {
  /** Where the moon should be. The canvas eases toward it rather than cutting. */
  phase: number;
  /** Marks it decorative for assistive tech; the copy beside it carries meaning. */
  label?: string;
}

/** The art is always this many characters. Fixing the grid and scaling the type
 * keeps the eclipse the same drawing at every window size — scaling the grid
 * instead would quietly redraw it, and the corona would gain detail on a big
 * monitor and lose it on a small one. */
const COLS = 54;
const ROWS = 22;

/** IBM Plex Mono advances 0.6em per character; the extra covers the tracking
 * applied in CSS. Measuring the real glyph would be more correct and would also
 * mean a layout read on every resize, for a number that does not change. */
const ADVANCE = 0.64;
const LINE_HEIGHT = 1.0;

/** How much of the remaining distance to the target phase to close each frame.
 * Low enough that a step change reads as the moon travelling, not jumping. */
const EASING = 0.045;

/** Below this, snap. Chasing the last thousandth forever keeps a rAF loop alive
 * and a laptop fan with it. */
const SETTLED = 0.0008;

/**
 * The eclipse, animated.
 *
 * Runs a requestAnimationFrame loop only while something is actually moving:
 * the corona drifts continuously, so the loop is alive whenever the panel is
 * visible — but it stops dead under `prefers-reduced-motion`, which renders a
 * single frame at the target phase and never asks for another.
 */
export function EclipseCanvas({ phase, label }: Props) {
  const [frame, setFrame] = useState<string[]>(() =>
    render({ width: COLS, height: ROWS, phase, time: 0 }),
  );
  const [fontSize, setFontSize] = useState(12);
  const box = useRef<HTMLDivElement | null>(null);
  const current = useRef(phase);
  const target = useRef(phase);

  target.current = phase;

  // Fit the fixed grid to whatever space the panel gives it. Layout effect so
  // the first paint is already the right size rather than visibly resizing.
  useLayoutEffect(() => {
    const element = box.current;
    if (!element) return;

    const fit = () => {
      const { width, height } = element.getBoundingClientRect();
      if (width === 0 || height === 0) return;
      // Held back from the edges: the corona should fade into space, and space
      // is the margin. Filling the panel edge to edge makes it a background.
      const byWidth = (width * 0.92) / (COLS * ADVANCE);
      const byHeight = (height * 0.92) / (ROWS * LINE_HEIGHT);
      setFontSize(Math.max(4, Math.min(byWidth, byHeight)));
    };

    fit();
    const observer = new ResizeObserver(fit);
    observer.observe(element);
    return () => observer.disconnect();
  }, []);

  useEffect(() => {
    const reduced = window.matchMedia("(prefers-reduced-motion: reduce)");

    if (reduced.matches) {
      current.current = target.current;
      setFrame(render({ width: COLS, height: ROWS, phase: target.current, time: 0 }));
      return;
    }

    let raf = 0;
    const started = performance.now();

    const tick = (now: number) => {
      const distance = target.current - current.current;
      current.current =
        Math.abs(distance) < SETTLED ? target.current : current.current + distance * EASING;

      setFrame(
        render({
          width: COLS,
          height: ROWS,
          phase: current.current,
          // Slow enough that the corona reads as alive rather than as noise.
          time: (now - started) / 2600,
        }),
      );
      raf = requestAnimationFrame(tick);
    };

    raf = requestAnimationFrame(tick);
    return () => cancelAnimationFrame(raf);
  }, []);

  return (
    <div className={styles.box} ref={box}>
      <pre
        className={styles.canvas}
        style={{ fontSize: `${fontSize}px` }}
        aria-label={label}
        role={label ? "img" : "presentation"}
      >
        {frame.join("\n")}
      </pre>
    </div>
  );
}
