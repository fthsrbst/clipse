import { useEffect } from "react";
import { useVirtualList } from "../hooks/use-virtual-list";
import { ClipRow } from "./clip-row";
import type { Clip } from "../types/ipc";
import styles from "./clip-list.module.css";

export interface ClipListProps {
  clips: Clip[];
  itemHeight: number;
  selectedIndex?: number;
  onActivate: (clip: Clip, index: number) => void;
  onTogglePin?: (clip: Clip) => void;
  onCopy?: (clip: Clip) => void;
  onDelete?: (clip: Clip) => void;
  compact?: boolean;
  onNearEnd?: () => void;
  /** Rows within the first nine get a Ctrl+N badge (popup only). */
  showShortcutBadges?: boolean;
}

/**
 * Windowed row rendering over `clips`. Never mounts more than a screenful
 * plus overscan, regardless of whether `clips` holds four rows or forty
 * thousand — see `hooks/use-virtual-list.ts` for the math.
 */
export function ClipList({
  clips,
  itemHeight,
  selectedIndex,
  onActivate,
  onTogglePin,
  onCopy,
  onDelete,
  compact = false,
  onNearEnd,
  showShortcutBadges = false,
}: ClipListProps) {
  const { containerRef, visible, totalHeight, onScroll } = useVirtualList(clips, {
    itemHeight,
    onNearEnd,
  });

  // Keep the selected row (popup keyboard nav) inside the scrolled viewport.
  useEffect(() => {
    if (selectedIndex === undefined) return;
    const container = containerRef.current;
    if (!container) return;
    const rowTop = selectedIndex * itemHeight;
    const rowBottom = rowTop + itemHeight;
    if (rowTop < container.scrollTop) {
      container.scrollTop = rowTop;
    } else if (rowBottom > container.scrollTop + container.clientHeight) {
      container.scrollTop = rowBottom - container.clientHeight;
    }
  }, [selectedIndex, itemHeight, containerRef]);

  return (
    <div
      ref={containerRef}
      className={styles.viewport}
      onScroll={onScroll}
      role="listbox"
      aria-label="Clipboard history"
    >
      <div className={styles.spacer} style={{ height: totalHeight }}>
        {visible.map(({ item, index }) => (
          <ClipRow
            key={item.id}
            clip={item}
            compact={compact}
            selected={selectedIndex === index}
            shortcutNumber={showShortcutBadges && index < 9 ? index + 1 : undefined}
            onActivate={() => onActivate(item, index)}
            onTogglePin={onTogglePin && (() => onTogglePin(item))}
            onCopy={onCopy && (() => onCopy(item))}
            onDelete={onDelete && (() => onDelete(item))}
            style={{ top: index * itemHeight, height: itemHeight }}
          />
        ))}
      </div>
    </div>
  );
}
