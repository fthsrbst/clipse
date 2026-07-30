import { getClipText, humanBytes } from "../lib/clip-content";
import { useClipPayload } from "../hooks/use-clip-payload";
import type { Clip, ClipFormat } from "../types/ipc";
import styles from "./peek-panel.module.css";

function formatLabel(format: ClipFormat): string {
  return typeof format === "string" ? format : format.Other;
}

export interface PeekPanelProps {
  clip: Clip;
  onClose: () => void;
}

/**
 * A clip in full, which the list cannot show.
 *
 * A row is one line, so until now a long clip's content had nowhere to be read
 * and its provenance — which application, which device, which digest — was
 * nowhere at all. The colophon underneath is deliberately the whole record
 * Clipse holds about a clip, digest included: a tool that watches everything
 * you copy should be legible about exactly what it kept.
 */
export function PeekPanel({ clip, onClose }: PeekPanelProps) {
  const { imageUrl, loading, tooLarge } = useClipPayload(clip);
  const text = getClipText(clip);
  const biggest = clip.payloads.reduce(
    (a, b) => (a && a.size >= b.size ? a : b),
    clip.payloads[0],
  );

  return (
    <aside className={styles.panel} aria-label="Clip detail">
      <header className={styles.head}>
        <span className={styles.kicker}>{clip.kind}</span>
        <button type="button" className={styles.close} aria-label="Close detail" onClick={onClose}>
          ✕
        </button>
      </header>

      <div className={styles.content}>
        {clip.kind === "image" ? (
          imageUrl ? (
            <img src={imageUrl} alt="" className={styles.image} />
          ) : loading ? (
            <p className={styles.note}>Reading…</p>
          ) : (
            /* Not an error state. Past the daemon's preview cap the size *is*
             * the answer — the clip is intact and pastes normally. */
            <p className={styles.note}>
              {tooLarge ? "Too large to preview" : "No preview"} · {humanBytes(biggest?.size ?? 0)}
            </p>
          )
        ) : (
          <pre className={styles.text}>{text ?? clip.preview}</pre>
        )}
      </div>

      <dl className={styles.meta}>
        <div className={styles.pair}>
          <dt>From</dt>
          <dd>{clip.source.app ?? "unknown app"}</dd>
        </div>
        <div className={styles.pair}>
          <dt>Device</dt>
          <dd>{clip.source.device_label}</dd>
        </div>
        <div className={styles.pair}>
          <dt>Copied</dt>
          <dd>{new Date(clip.created_at_ms).toLocaleString()}</dd>
        </div>
        {clip.payloads.map((p) => (
          <div className={styles.pair} key={`${formatLabel(p.format)}-${p.digest}`}>
            <dt>{formatLabel(p.format)}</dt>
            <dd>{humanBytes(p.size)}</dd>
          </div>
        ))}
        <div className={styles.pair}>
          <dt>Hash</dt>
          {/* Truncated to what the eye can compare, with the whole digest one
           * hover away. */}
          <dd title={clip.hash}>{clip.hash.slice(0, 12)}</dd>
        </div>
      </dl>
    </aside>
  );
}
