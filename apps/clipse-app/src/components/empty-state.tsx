import { AsciiLogo } from "./ascii-logo";
import { EclipseCanvas } from "./eclipse-canvas";
import styles from "./empty-state.module.css";

export interface EmptyStateProps {
  title: string;
  description?: string;
  action?: React.ReactNode;
  /** Loading and offline states reuse this layout with the mark animated or
   * swapped for a status glyph — see `DaemonOfflineState`. */
  animated?: boolean;
}

export function EmptyState({ title, description, action, animated = false }: EmptyStateProps) {
  return (
    <div className={styles.wrap}>
      {/* Loading gets the computed field, which drifts; a settled empty state
       * gets the fixed mark. An animation on a state that is not waiting for
       * anything says something is happening when nothing is. */}
      {animated ? (
        <div className={styles.field}>
          <EclipseCanvas phase={0.5} />
        </div>
      ) : (
        <AsciiLogo variant="mark" cell={7} className={styles.mark} />
      )}
      <p className={styles.title}>{title}</p>
      {description && <p className={styles.description}>{description}</p>}
      {action && <div className={styles.action}>{action}</div>}
    </div>
  );
}
