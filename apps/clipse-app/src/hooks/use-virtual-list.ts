import { useCallback, useEffect, useRef, useState } from "react";
import { computeVirtualRange } from "../lib/virtual-list";

export interface UseVirtualListOptions {
  itemHeight: number;
  overscan?: number;
  /** Called when the scroll position gets within `itemHeight * 6` of the
   * bottom of the currently loaded items — the History window's pagination
   * hook uses this to fetch the next page. */
  onNearEnd?: () => void;
}

export interface VirtualListEntry<T> {
  item: T;
  index: number;
}

export function useVirtualList<T>(items: readonly T[], options: UseVirtualListOptions) {
  const { itemHeight, overscan = 6, onNearEnd } = options;
  const containerRef = useRef<HTMLDivElement | null>(null);
  const [scrollTop, setScrollTop] = useState(0);
  const [containerHeight, setContainerHeight] = useState(0);

  useEffect(() => {
    const node = containerRef.current;
    if (!node) return;
    const observer = new ResizeObserver(([entry]) => {
      setContainerHeight(entry.contentRect.height);
    });
    observer.observe(node);
    setContainerHeight(node.clientHeight);
    return () => observer.disconnect();
  }, []);

  const range = computeVirtualRange({
    itemCount: items.length,
    itemHeight,
    containerHeight,
    scrollTop,
    overscan,
  });

  const onScroll = useCallback(
    (event: React.UIEvent<HTMLDivElement>) => {
      const top = event.currentTarget.scrollTop;
      setScrollTop(top);
      const remaining = event.currentTarget.scrollHeight - top - event.currentTarget.clientHeight;
      if (onNearEnd && remaining < itemHeight * 6) onNearEnd();
    },
    [itemHeight, onNearEnd],
  );

  const visible: Array<VirtualListEntry<T>> = [];
  for (let i = range.startIndex; i < range.endIndex; i++) {
    visible.push({ item: items[i], index: i });
  }

  return {
    containerRef,
    visible,
    totalHeight: range.totalHeight,
    offsetTop: range.offsetTop,
    onScroll,
  };
}
