/** Relative timestamp formatting for clip rows: "just now" through "3h ago",
 * falling back to a calendar date once a clip is old enough that a relative
 * offset stops being useful at a glance. */

const SECOND = 1_000;
const MINUTE = 60 * SECOND;
const HOUR = 60 * MINUTE;
const DAY = 24 * HOUR;
const WEEK = 7 * DAY;

/**
 * @param timestampMs Milliseconds since epoch (matches `Clip.created_at_ms`).
 * @param now Injectable for tests; defaults to the real current time.
 */
export function formatRelativeTime(timestampMs: number, now: number = Date.now()): string {
  const diff = now - timestampMs;

  // A timestamp from the future (clock skew between devices) reads as "just
  // now" rather than a nonsensical negative duration.
  if (diff <= 0) return "just now";
  if (diff < 10 * SECOND) return "just now";
  if (diff < MINUTE) return `${Math.floor(diff / SECOND)}s ago`;
  if (diff < HOUR) return `${Math.floor(diff / MINUTE)}m ago`;
  if (diff < DAY) return `${Math.floor(diff / HOUR)}h ago`;
  if (diff < WEEK) return `${Math.floor(diff / DAY)}d ago`;

  const date = new Date(timestampMs);
  const sameYear = date.getFullYear() === new Date(now).getFullYear();
  return date.toLocaleDateString("en-US", {
    month: "short",
    day: "numeric",
    year: sameYear ? undefined : "numeric",
  });
}
