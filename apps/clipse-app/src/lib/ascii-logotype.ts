/**
 * The identity, drawn in characters.
 *
 * These are deterministic grids, unlike `eclipse-ascii.ts`, which renders the
 * *field* that animates and may legitimately differ frame to frame. A logo may
 * not: it has to be the same drawing in the spine, in an empty state and in the
 * colophon, including in the places that never animate.
 *
 * Every row of a grid is the same length. `ascii-logotype.test.ts` enforces
 * that, because a row one character short leans the whole mark by a fraction
 * of a cell — visible, but not obviously *wrong*, which is the worst kind of
 * broken.
 */

/**
 * The eclipse: a disc with the moon taking a bite out of it.
 *
 * Nine rows is the smallest that still reads as two overlapping circles rather
 * than as texture, and the ramp characters are the same ones `eclipse-ascii.ts`
 * uses for the corona, so the fixed mark and the animated field are visibly the
 * same drawing.
 */
export const ECLIPSE_MARK: readonly string[] = [
  "  ,:;=!*!=;:,  ",
  " ;=*#      #*= ",
  ":*#          #*",
  "=*            *",
  "!#            #",
  "=*            *",
  ":*#          #*",
  " ;=*#      #*= ",
  "  ,:;=!*!=;:,  ",
];

/**
 * `CLIPSE` as block letterforms, five cells wide and six tall.
 *
 * Composed rather than written out as six long literals. Hand-counting a
 * 35-character row six times is a guaranteed source of exactly the off-by-one
 * the test above exists to catch, and a letterform is easier to judge on its
 * own than buried in a line of five others.
 *
 * The counters — the holes in C, P, S and E — are kept a full cell wide. At the
 * sizes this is set (down to about 4.5px per cell) a one-cell counter closes up
 * and the letter reads as a filled block.
 *
 * `I` is three cells rather than five, which is also what a proportional face
 * would do with it. Set at five, its bottom serif ran flush into `L`'s foot and
 * the pair read as one long bar instead of two letters.
 */
const LETTERS: Record<string, readonly string[]> = {
  C: [" ### ", "#   #", "#    ", "#    ", "#   #", " ### "],
  L: ["#    ", "#    ", "#    ", "#    ", "#    ", "#####"],
  I: ["###", " # ", " # ", " # ", " # ", "###"],
  P: ["#### ", "#   #", "#   #", "#### ", "#    ", "#    "],
  S: [" ####", "#    ", " ### ", "    #", "    #", "#### "],
  E: ["#####", "#    ", "#### ", "#    ", "#    ", "#####"],
};

const WORDMARK_ROWS = 6;

function compose(word: string): readonly string[] {
  const glyphs = [...word].map((char) => {
    const glyph = LETTERS[char];
    if (!glyph) throw new Error(`no letterform for ${char}`);
    return glyph;
  });

  return Array.from({ length: WORDMARK_ROWS }, (_, row) =>
    glyphs.map((glyph) => glyph[row]).join(" "),
  );
}

export const CLIPSE_WORDMARK: readonly string[] = compose("CLIPSE");
