/**
 * Pure windowing math for the History list.
 *
 * History is unbounded — it can hold years of clips — so the list never
 * renders more than a small window of rows around the viewport, regardless
 * of how many thousands of clips are loaded. This file has no React in it on
 * purpose: the math is what needs to be right, and it is easiest to get
 * right (and to test) as plain arithmetic over numbers.
 */

export interface VirtualRange {
  /** First rendered index, inclusive. */
  startIndex: number;
  /** Last rendered index, exclusive. */
  endIndex: number;
  /** Pixels to push the rendered window down so row `startIndex` lands where
   * it would if every prior row were actually in the DOM. */
  offsetTop: number;
  /** Total scrollable height if every row were rendered — what the spacer
   * element below/above the window needs to add up to. */
  totalHeight: number;
}

export interface VirtualRangeParams {
  itemCount: number;
  itemHeight: number;
  containerHeight: number;
  scrollTop: number;
  /** Extra rows rendered above and below the visible window, so a fast
   * scroll doesn't flash blank space before the next paint catches up. */
  overscan?: number;
}

export function computeVirtualRange(params: VirtualRangeParams): VirtualRange {
  const { itemCount, itemHeight, containerHeight, scrollTop, overscan = 4 } = params;
  const totalHeight = Math.max(0, itemCount) * Math.max(0, itemHeight);

  if (itemCount <= 0 || itemHeight <= 0) {
    return { startIndex: 0, endIndex: 0, offsetTop: 0, totalHeight };
  }

  const clampedScrollTop = Math.max(0, scrollTop);
  const firstVisible = Math.floor(clampedScrollTop / itemHeight);
  const visibleRows = Math.max(1, Math.ceil(containerHeight / itemHeight));

  const startIndex = Math.max(0, firstVisible - overscan);
  const endIndex = Math.min(itemCount, firstVisible + visibleRows + overscan);

  return {
    startIndex,
    endIndex,
    offsetTop: startIndex * itemHeight,
    totalHeight,
  };
}
