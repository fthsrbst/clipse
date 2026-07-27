import { CLIP_TYPE_FILTERS, type ClipTypeFilter } from "../lib/popup-reducer";
import { FileIcon, ImageIcon, LinkIcon, TextIcon } from "./icons";
import type { IconProps } from "./icons";
import styles from "./type-filter-tabs.module.css";

const LABELS: Record<ClipTypeFilter, string> = {
  all: "All",
  text: "Text",
  image: "Images",
  files: "Files",
  link: "Links",
};

const ICONS: Partial<Record<ClipTypeFilter, (props: IconProps) => React.JSX.Element>> = {
  text: TextIcon,
  image: ImageIcon,
  files: FileIcon,
  link: LinkIcon,
};

export interface TypeFilterTabsProps {
  value: ClipTypeFilter;
  onChange: (filter: ClipTypeFilter) => void;
  /** Compact mode drops the labels down to icons only, for the popup. */
  compact?: boolean;
}

export function TypeFilterTabs({ value, onChange, compact = false }: TypeFilterTabsProps) {
  return (
    <div className={styles.tabs} role="tablist" aria-label="Filter by clip type">
      {CLIP_TYPE_FILTERS.map((filter) => {
        const Icon = ICONS[filter];
        const active = filter === value;
        return (
          <button
            key={filter}
            type="button"
            role="tab"
            aria-selected={active}
            className={active ? `${styles.tab} ${styles.active}` : styles.tab}
            onClick={() => onChange(filter)}
          >
            {Icon && <Icon size={14} />}
            {!compact && <span>{LABELS[filter]}</span>}
            {compact && !Icon && <span>{LABELS[filter]}</span>}
          </button>
        );
      })}
    </div>
  );
}
