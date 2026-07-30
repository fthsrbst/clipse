import { looksLikeLink } from "../lib/clip-content";
import type { Clip } from "../types/ipc";

/**
 * A clip's kind, as one character.
 *
 * A history row is a line of set text, and an SVG icon in it is a foreign
 * object sitting on the baseline. The popup keeps `KindIcon`: it is a different
 * surface, summoned mid-task, and is deliberately out of scope here.
 */
export function KindGlyph({ clip }: { clip: Clip }) {
  const glyph =
    clip.kind === "image" ? "▤" : clip.kind === "files" ? "▧" : looksLikeLink(clip) ? "↗" : "—";

  return (
    <span aria-hidden="true" data-kind={clip.kind}>
      {glyph}
    </span>
  );
}
