/**
 * The eclipse motif: a filled disc with a crescent bite taken out by an
 * offset disc — the app mark, and the shape behind every empty/loading
 * state. Inlined (not an <img>) so gradient/mask ids can be namespaced per
 * instance with `useId`, and so `animated` can drive the corona with plain
 * CSS custom properties tied to the motion tokens.
 *
 * Source of truth for the geometry: `assets/brand/clipse-mark.svg` and
 * `clipse-mark-mono.svg` at the repo root.
 */

import { useId } from "react";
import styles from "./eclipse-mark.module.css";

export interface EclipseMarkProps {
  size?: number;
  /** `full` uses the amber corona gradient; `mono` renders in
   * `currentColor` for placement on already-colored surfaces. */
  variant?: "full" | "mono";
  /** Slow pulse + drift, for a loading state. Respects
   * `prefers-reduced-motion` via the shared duration tokens. */
  animated?: boolean;
  className?: string;
}

export function EclipseMark({ size = 96, variant = "full", animated = false, className }: EclipseMarkProps) {
  const uid = useId().replace(/[:]/g, "");
  const coronaId = `corona-${uid}`;
  const cutoutId = `cutout-${uid}`;

  const classes = [styles.mark, animated ? styles.animated : "", className].filter(Boolean).join(" ");

  return (
    <svg
      viewBox="0 0 512 512"
      width={size}
      height={size}
      className={classes}
      role="img"
      aria-label="Clipse"
    >
      <defs>
        {variant === "full" && (
          <linearGradient id={coronaId} x1="0" y1="0" x2="1" y2="1">
            <stop offset="0" stopColor="var(--amber-400)" />
            <stop offset="1" stopColor="var(--amber-600)" />
          </linearGradient>
        )}
        <mask id={cutoutId} x="0" y="0" width="512" height="512" maskUnits="userSpaceOnUse">
          <circle cx="248" cy="258" r="150" fill="#fff" />
          <circle className={styles.bite} cx="310" cy="198" r="139" fill="#000" />
        </mask>
      </defs>
      <circle
        className={styles.corona}
        cx="250"
        cy="256"
        r="190"
        fill="none"
        stroke={variant === "full" ? `url(#${coronaId})` : "currentColor"}
        strokeWidth="37.333"
      />
      <circle
        cx="248"
        cy="258"
        r="150"
        fill={variant === "full" ? "var(--color-canvas)" : "currentColor"}
        mask={`url(#${cutoutId})`}
      />
    </svg>
  );
}
