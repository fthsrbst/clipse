import { useState } from "react";
import { CloseIcon } from "./icons";
import styles from "./tag-list-input.module.css";

export interface TagListInputProps {
  values: string[];
  onChange: (values: string[]) => void;
  placeholder?: string;
}

/** A small add/remove list — used for the blocked-apps setting. Each entry
 * is expected to be a process/app identifier the user types by hand, so
 * this is deliberately just a text field plus Enter, not an app picker. */
export function TagListInput({ values, onChange, placeholder = "app-name.exe" }: TagListInputProps) {
  const [draft, setDraft] = useState("");

  function commit() {
    const trimmed = draft.trim();
    if (trimmed.length > 0 && !values.includes(trimmed)) {
      onChange([...values, trimmed]);
    }
    setDraft("");
  }

  return (
    <div className={styles.wrap}>
      <div className={styles.tags}>
        {values.map((value) => (
          <span key={value} className={styles.tag}>
            {value}
            <button
              type="button"
              className={styles.remove}
              aria-label={`Remove ${value}`}
              onClick={() => onChange(values.filter((v) => v !== value))}
            >
              <CloseIcon size={11} />
            </button>
          </span>
        ))}
        {values.length === 0 && <span className={styles.empty}>No apps blocked</span>}
      </div>
      <input
        type="text"
        className={styles.input}
        value={draft}
        placeholder={placeholder}
        onChange={(e) => setDraft(e.target.value)}
        onKeyDown={(e) => {
          if (e.key === "Enter") {
            e.preventDefault();
            commit();
          }
        }}
        onBlur={commit}
      />
    </div>
  );
}
