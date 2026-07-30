import { CLIPSE_WORDMARK, ECLIPSE_MARK } from "../lib/ascii-logotype";
import styles from "./ascii-logo.module.css";

/**
 * Below this many pixels per character cell the nine-row eclipse stops being
 * two circles and becomes a smudge.
 *
 * This is the constraint that killed the previous attempt at a character mark,
 * and the answer is not a cleverer grid — it is not asking the grid for
 * something it cannot give. Anywhere a mark must survive 16px (a tray icon, an
 * OS icon) stays an SVG, because an operating system will not take a character
 * grid either.
 */
const MIN_CELL_PX = 4.5;

export interface AsciiLogoProps {
  variant?: "mark" | "wordmark" | "lockup";
  /** Height of one character cell in px. The grid scales from this alone, so
   * it is the same drawing at every size rather than a redrawn one. */
  cell?: number;
  className?: string;
}

export function AsciiLogo({ variant = "mark", cell = 6, className }: AsciiLogoProps) {
  const size = Math.max(MIN_CELL_PX, cell);
  const rows =
    variant === "mark"
      ? ECLIPSE_MARK
      : variant === "wordmark"
        ? CLIPSE_WORDMARK
        : [...ECLIPSE_MARK, "", ...CLIPSE_WORDMARK];

  return (
    <pre
      className={[styles.logo, className].filter(Boolean).join(" ")}
      style={{ fontSize: `${size}px` }}
      role="img"
      aria-label="Clipse"
    >
      {rows.join("\n")}
    </pre>
  );
}
