import { getClipImageDataUrl, hasBlobPayload, humanBytes } from "../lib/clip-content";
import { formatRelativeTime } from "../lib/relative-time";
import { KindIcon } from "./kind-icon";
import { KindGlyph } from "./kind-glyph";
import { CopyIcon, PinFilledIcon, PinIcon, TrashIcon } from "./icons";
import type { Clip } from "../types/ipc";
import styles from "./clip-row.module.css";

export interface ClipRowProps {
  clip: Clip;
  /** History window: click selects/focuses the row. Popup: click pastes. */
  onActivate: () => void;
  selected?: boolean;
  /** 1-based badge for the popup's Ctrl+1..9 shortcuts (only the first nine
   * visible rows get one). */
  shortcutNumber?: number;
  onTogglePin?: () => void;
  onCopy?: () => void;
  onDelete?: () => void;
  /** Popup rows omit the per-row action buttons — Enter/click already does
   * the one thing the popup is for. */
  compact?: boolean;
  style?: React.CSSProperties;
}

export function ClipRow({
  clip,
  onActivate,
  selected = false,
  shortcutNumber,
  onTogglePin,
  onCopy,
  onDelete,
  compact = false,
  style,
}: ClipRowProps) {
  const imageSrc = clip.kind === "image" ? getClipImageDataUrl(clip) : undefined;
  const isUnfetchedBlob = clip.kind === "image" && !imageSrc && hasBlobPayload(clip);

  const rowClass = [styles.row, selected ? styles.selected : "", compact ? styles.compact : ""]
    .filter(Boolean)
    .join(" ");

  return (
    <div
      className={rowClass}
      style={style}
      role="option"
      aria-selected={selected}
      tabIndex={-1}
      onClick={onActivate}
    >
      {shortcutNumber !== undefined && shortcutNumber <= 9 && (
        <span className={styles.shortcut}>{shortcutNumber}</span>
      )}

      {/* The popup keeps the icon: it is a denser, faster surface where a glyph
       * at this size would be harder to pick out at a glance. */}
      <span className={styles.kind}>
        {compact ? <KindIcon clip={clip} size={15} /> : <KindGlyph clip={clip} />}
      </span>

      {imageSrc ? (
        <img src={imageSrc} alt="" className={styles.thumb} />
      ) : isUnfetchedBlob ? (
        <div className={styles.thumbPlaceholder}>{humanBytes(clip.payloads[0]?.size ?? 0)}</div>
      ) : null}

      {/* In the history the row is set as columns, so the device and the time
       * line up down the window and can be read as a list rather than as a
       * caption under each preview. The popup stays stacked: it is 420px wide
       * and columns there would leave no room for the preview itself. */}
      <div className={styles.body}>
        <p className={styles.preview}>{clip.preview}</p>
        {compact && (
          <div className={styles.meta}>
            <span>{formatRelativeTime(clip.created_at_ms)}</span>
            <span className={styles.dot} aria-hidden="true">
              ·
            </span>
            <span className={styles.device}>{clip.source.device_label}</span>
          </div>
        )}
      </div>

      {!compact && <span className={styles.device}>{clip.source.device_label}</span>}
      {!compact && <span className={styles.time}>{formatRelativeTime(clip.created_at_ms)}</span>}

      {/* Hangs in the row's left margin, outside the text column. The overhang
       * is the point: a pinned row breaks the left edge of the block, which
       * reads down a long list in a way an inline icon does not. */}
      {clip.pinned && <span className={styles.pinTick} aria-hidden="true" />}

      {!compact && (
        <div className={styles.actions}>
          {onTogglePin && (
            <button
              type="button"
              className={styles.actionBtn}
              aria-label={clip.pinned ? "Unpin" : "Pin"}
              aria-pressed={clip.pinned}
              onClick={(e) => {
                e.stopPropagation();
                onTogglePin();
              }}
            >
              {clip.pinned ? <PinFilledIcon size={14} /> : <PinIcon size={14} />}
            </button>
          )}
          {onCopy && (
            <button
              type="button"
              className={styles.actionBtn}
              aria-label="Copy to clipboard"
              onClick={(e) => {
                e.stopPropagation();
                onCopy();
              }}
            >
              <CopyIcon size={14} />
            </button>
          )}
          {onDelete && (
            <button
              type="button"
              className={`${styles.actionBtn} ${styles.danger}`}
              aria-label="Delete"
              onClick={(e) => {
                e.stopPropagation();
                onDelete();
              }}
            >
              <TrashIcon size={14} />
            </button>
          )}
        </div>
      )}
    </div>
  );
}
