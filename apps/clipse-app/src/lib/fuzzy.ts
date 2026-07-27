/**
 * A small, dependency-free fuzzy matcher for the hotkey popup.
 *
 * The popup filters a locally-held buffer of recent clips as the user types,
 * with no round trip to the daemon per keystroke (the daemon's FTS `search`
 * command is used by the History window instead, where a network-shaped
 * round trip is acceptable). This needs to be fast and to rank "obviously
 * right" matches above scattered ones, not just find any match.
 */

export interface FuzzyMatch {
  /** Higher is a better match. Only meaningful relative to other scores from
   * the same query. */
  score: number;
  /** Indices into `text` that matched a query character, in order. Useful
   * for highlighting. */
  indices: number[];
}

const WORD_BOUNDARY = /[^a-z0-9]/i;

/**
 * Subsequence match of `query` against `text`, case-insensitive.
 *
 * Returns `null` when `query` is not a subsequence of `text` at all (a real
 * non-match, not just a low score) so callers can filter rather than sort
 * around it.
 */
export function fuzzyMatch(query: string, text: string): FuzzyMatch | null {
  if (query.length === 0) {
    return { score: 0, indices: [] };
  }
  if (text.length === 0) {
    return null;
  }

  const q = query.toLowerCase();
  const t = text.toLowerCase();

  const indices: number[] = [];
  let qi = 0;
  let score = 0;
  let previousMatch = -2;
  let run = 0;

  for (let ti = 0; ti < t.length && qi < q.length; ti++) {
    if (t[ti] !== q[qi]) continue;

    const consecutive = previousMatch === ti - 1;
    run = consecutive ? run + 1 : 1;

    let charScore = 10;
    // Runs of consecutive matches are worth increasingly more than the same
    // characters scattered across the string — "abc" beats "a-b-c".
    if (consecutive) charScore += run * 6;
    // A match right at the start of a word (or the string) reads as
    // intentional, e.g. matching "cl" against the "C" in "Clipse".
    if (ti === 0 || WORD_BOUNDARY.test(t[ti - 1])) charScore += 12;

    score += charScore;
    indices.push(ti);
    previousMatch = ti;
    qi++;
  }

  if (qi < q.length) {
    return null; // some query character never found in order — not a match
  }

  // Tie-breakers: matches packed into a shorter span, and starting earlier
  // in the string, both read as a better match than the same characters
  // found late or spread thin.
  const first = indices[0];
  const last = indices[indices.length - 1];
  const span = last - first + 1;
  const slack = span - query.length; // 0 when every match is consecutive
  score -= slack * 2;
  score -= first * 1;

  return { score, indices };
}

export interface FuzzyHit<T> {
  item: T;
  score: number;
  indices: number[];
}

/**
 * Filter and rank `items` against `query`, best match first. Non-matches are
 * dropped rather than sorted to the bottom. An empty (or whitespace-only)
 * query returns every item, unscored, in its original order — the popup's
 * "no filter yet" state.
 */
export function fuzzyFilter<T>(
  query: string,
  items: readonly T[],
  getText: (item: T) => string,
): Array<FuzzyHit<T>> {
  const trimmed = query.trim();
  if (trimmed.length === 0) {
    return items.map((item) => ({ item, score: 0, indices: [] }));
  }

  const hits: Array<FuzzyHit<T>> = [];
  for (const item of items) {
    const result = fuzzyMatch(trimmed, getText(item));
    if (result) hits.push({ item, score: result.score, indices: result.indices });
  }
  hits.sort((a, b) => b.score - a.score);
  return hits;
}
