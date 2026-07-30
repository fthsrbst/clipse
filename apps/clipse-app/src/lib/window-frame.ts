/**
 * The parts of a frameless window that Win32 stops providing for free.
 *
 * `decorations: false` removes the OS title bar — which on Windows sat directly
 * above the masthead as a second header — and takes the resize border and the
 * double-click-to-maximize target with it. Both have to be put back by hand.
 *
 * The mapping below is the half that fails silently: a misspelled direction
 * does not throw, it makes one edge dead, and nobody notices until they happen
 * to grab that edge.
 */

export type ResizeEdge = "n" | "s" | "e" | "w" | "ne" | "nw" | "se" | "sw";

export const RESIZE_EDGES: readonly ResizeEdge[] = [
  "n",
  "s",
  "e",
  "w",
  "ne",
  "nw",
  "se",
  "sw",
];

const DIRECTIONS: Record<ResizeEdge, string> = {
  n: "North",
  s: "South",
  e: "East",
  w: "West",
  ne: "NorthEast",
  nw: "NorthWest",
  se: "SouthEast",
  sw: "SouthWest",
};

export function resizeDirection(edge: ResizeEdge): string {
  return DIRECTIONS[edge];
}
