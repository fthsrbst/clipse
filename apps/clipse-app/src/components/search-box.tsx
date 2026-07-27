import { forwardRef } from "react";
import { SearchIcon } from "./icons";
import styles from "./search-box.module.css";

export interface SearchBoxProps {
  value: string;
  onChange: (value: string) => void;
  placeholder?: string;
  onKeyDown?: React.KeyboardEventHandler<HTMLInputElement>;
  autoFocus?: boolean;
}

export const SearchBox = forwardRef<HTMLInputElement, SearchBoxProps>(function SearchBox(
  { value, onChange, placeholder = "Search clips…", onKeyDown, autoFocus },
  ref,
) {
  return (
    <div className={styles.wrap}>
      <SearchIcon className={styles.icon} size={16} />
      <input
        ref={ref}
        type="text"
        className={styles.input}
        value={value}
        onChange={(e) => onChange(e.target.value)}
        onKeyDown={onKeyDown}
        placeholder={placeholder}
        autoFocus={autoFocus}
        aria-label="Search clipboard history"
        spellCheck={false}
        autoComplete="off"
      />
    </div>
  );
});
