import { gsap } from "gsap";

/**
 * The motion system.
 *
 * These numbers are the same ones in `styles/tokens.css`. They are duplicated
 * rather than read from the cascade because GSAP needs seconds and cubic-bezier
 * arrays, not CSS strings, and resolving custom properties per tween would mean
 * a layout read on every animation. If one side changes, change the other.
 *
 * Everything here respects `prefers-reduced-motion`: `duration()` collapses to
 * zero, so timelines still run and still fire their callbacks — the end state
 * is simply reached immediately. That is deliberate. Skipping the timeline
 * entirely would leave elements at their `from` values whenever an animation
 * was also doing the work of setting up.
 */

export const EASE = {
  out: "power3.out",
  exit: "power2.in",
  inOut: "power2.inOut",
  spring: "back.out(1.7)",
} as const;

const D = {
  instant: 0.09,
  fast: 0.16,
  base: 0.28,
  slow: 0.52,
  glacial: 0.9,
} as const;

export const STAGGER = 0.032;

let reduced: boolean | null = null;

export function prefersReducedMotion(): boolean {
  // Cached: this is read on every tween and `matchMedia` is not free.
  if (reduced === null) {
    reduced = window.matchMedia("(prefers-reduced-motion: reduce)").matches;
  }
  return reduced;
}

/** A duration from the scale, in seconds — or zero if motion is unwelcome. */
export function duration(name: keyof typeof D): number {
  return prefersReducedMotion() ? 0 : D[name];
}

export function stagger(multiplier = 1): number {
  return prefersReducedMotion() ? 0 : STAGGER * multiplier;
}

/**
 * The house entrance: rise and fade, staggered down a group.
 *
 * Used for every "this screen just became the screen" moment, so the product
 * has one way of arriving rather than a different one per view.
 */
export function enter(
  targets: gsap.TweenTarget,
  options: { delay?: number; distance?: number; each?: number } = {},
): gsap.core.Tween {
  const { delay = 0, distance = 18, each = 1 } = options;
  return gsap.fromTo(
    targets,
    { opacity: 0, y: distance },
    {
      opacity: 1,
      y: 0,
      duration: duration("slow"),
      ease: EASE.out,
      stagger: stagger(each),
      delay,
      clearProps: "transform",
    },
  );
}

/** The house exit. Shorter than the entrance, and it does not overshoot. */
export function exit(targets: gsap.TweenTarget, distance = 10): gsap.core.Tween {
  return gsap.to(targets, {
    opacity: 0,
    y: -distance,
    duration: duration("fast"),
    ease: EASE.exit,
  });
}

/**
 * Count a number up to its value.
 *
 * Only worth doing for figures that are the point of the screen — the clip
 * count, a byte total. Animating every number on a page is how a dashboard
 * starts feeling like a slot machine.
 */
export function countTo(
  element: HTMLElement,
  to: number,
  format: (n: number) => string = (n) => String(Math.round(n)),
): gsap.core.Tween {
  const state = { value: Number(element.dataset.countFrom ?? 0) };
  return gsap.to(state, {
    value: to,
    duration: duration("slow"),
    ease: EASE.out,
    onUpdate: () => {
      element.textContent = format(state.value);
    },
    onComplete: () => {
      element.dataset.countFrom = String(to);
    },
  });
}

export { gsap };
