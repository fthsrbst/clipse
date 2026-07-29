import styles from "./ascii-mark.module.css";

/**
 * The Clipse mark: a disc with the moon bitten out of it.
 *
 * Drawn as geometry rather than as characters. The product's artwork *is* a
 * character grid, and a three-row ASCII eclipse was the obvious thing to put
 * here — but at the size a mark sits beside a wordmark it collapsed into an
 * illegible smudge. A mark has to survive 16px; the ASCII field cannot.
 *
 * `aria-hidden` because the wordmark next to it already says Clipse.
 */
export function AsciiMark() {
  return (
    <svg
      className={styles.mark}
      viewBox="0 0 32 32"
      role="presentation"
      aria-hidden="true"
      focusable="false"
    >
      {/* The bite is a mask rather than a second filled circle, so the mark
       * works on any background instead of only on the canvas colour. */}
      <mask id="clipse-eclipse">
        <rect width="32" height="32" fill="black" />
        <circle cx="16" cy="16" r="13" fill="white" />
        <circle cx="23.2" cy="16" r="11.2" fill="black" />
      </mask>
      <rect width="32" height="32" fill="currentColor" mask="url(#clipse-eclipse)" />
    </svg>
  );
}
