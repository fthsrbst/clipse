import { EclipseMark } from "./eclipse-mark";
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
      <EclipseMark size={64} animated={animated} className={styles.mark} />
      <p className={styles.title}>{title}</p>
      {description && <p className={styles.description}>{description}</p>}
      {action && <div className={styles.action}>{action}</div>}
    </div>
  );
}
