import { describe, expect, it } from "vitest";
import { formatRelativeTime } from "./relative-time";

const NOW = Date.parse("2026-07-27T12:00:00.000Z");

describe("formatRelativeTime", () => {
  it("reports a just-captured clip as just now", () => {
    expect(formatRelativeTime(NOW, NOW)).toBe("just now");
    expect(formatRelativeTime(NOW - 9_000, NOW)).toBe("just now");
  });

  it("clamps future timestamps (clock skew) to just now", () => {
    expect(formatRelativeTime(NOW + 60_000, NOW)).toBe("just now");
  });

  it("formats seconds", () => {
    expect(formatRelativeTime(NOW - 45_000, NOW)).toBe("45s ago");
  });

  it("formats minutes", () => {
    expect(formatRelativeTime(NOW - 5 * 60_000, NOW)).toBe("5m ago");
    expect(formatRelativeTime(NOW - 59 * 60_000, NOW)).toBe("59m ago");
  });

  it("formats hours", () => {
    expect(formatRelativeTime(NOW - 3 * 3_600_000, NOW)).toBe("3h ago");
    expect(formatRelativeTime(NOW - 23 * 3_600_000, NOW)).toBe("23h ago");
  });

  it("formats days", () => {
    expect(formatRelativeTime(NOW - 2 * 86_400_000, NOW)).toBe("2d ago");
    expect(formatRelativeTime(NOW - 6 * 86_400_000, NOW)).toBe("6d ago");
  });

  it("falls back to a calendar date without a year in the same year", () => {
    const eightDaysAgo = NOW - 8 * 86_400_000;
    expect(formatRelativeTime(eightDaysAgo, NOW)).toBe("Jul 19");
  });

  it("includes the year for a date in a previous year", () => {
    const lastYear = Date.parse("2025-01-15T12:00:00.000Z");
    expect(formatRelativeTime(lastYear, NOW)).toBe("Jan 15, 2025");
  });

  it("sits right at the minute/hour boundary correctly", () => {
    expect(formatRelativeTime(NOW - 60_000, NOW)).toBe("1m ago");
    expect(formatRelativeTime(NOW - 3_600_000, NOW)).toBe("1h ago");
    expect(formatRelativeTime(NOW - 86_400_000, NOW)).toBe("1d ago");
  });
});
