import styles from "./ascii-mark.module.css";

/**
 * The mark, drawn in characters.
 *
 * Three rows of monospace rather than an SVG, because the product's own
 * artwork is a character grid and a vector eclipse beside it would be a second
 * logo. It is small enough to sit on a baseline next to the wordmark and still
 * read as a disc with a bright limb on one side — the moment before totality.
 *
 * Marked `aria-hidden`: the wordmark next to it already says Clipse, and a
 * screen reader spelling out punctuation is worse than silence.
 */
const ROWS = [",=*#*=,", "=*   #@", ",=*#*=,"];

export function AsciiMark() {
  return (
    <pre className={styles.mark} aria-hidden="true">
      {ROWS.join("\n")}
    </pre>
  );
}
