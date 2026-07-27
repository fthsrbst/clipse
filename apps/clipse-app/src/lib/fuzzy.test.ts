import { describe, expect, it } from "vitest";
import { fuzzyFilter, fuzzyMatch } from "./fuzzy";

describe("fuzzyMatch", () => {
  it("matches a contiguous substring", () => {
    const result = fuzzyMatch("clip", "Clipse keeps everything");
    expect(result).not.toBeNull();
    expect(result?.indices).toEqual([0, 1, 2, 3]);
  });

  it("matches a scattered subsequence", () => {
    const result = fuzzyMatch("cse", "Clipse");
    expect(result).not.toBeNull();
    expect(result?.indices).toEqual([0, 4, 5]);
  });

  it("is case-insensitive", () => {
    expect(fuzzyMatch("CLIP", "clipboard")).not.toBeNull();
    expect(fuzzyMatch("clip", "CLIPBOARD")).not.toBeNull();
  });

  it("returns an empty match for an empty query", () => {
    expect(fuzzyMatch("", "anything")).toEqual({ score: 0, indices: [] });
  });

  it("returns null when a character is missing entirely", () => {
    expect(fuzzyMatch("xyz", "clipboard")).toBeNull();
  });

  it("returns null when characters are present but out of order", () => {
    // 'p' before 'c' never occurs in "clip" in that order after the first c
    expect(fuzzyMatch("pc", "clip")).toBeNull();
  });

  it("returns null against an empty target unless the query is also empty", () => {
    expect(fuzzyMatch("a", "")).toBeNull();
  });

  it("ranks a contiguous prefix match above a scattered match", () => {
    const tight = fuzzyMatch("cli", "clipboard");
    const loose = fuzzyMatch("cli", "cap link item");
    expect(tight).not.toBeNull();
    expect(loose).not.toBeNull();
    expect(tight!.score).toBeGreaterThan(loose!.score);
  });

  it("ranks a word-start match above a mid-word match of the same length", () => {
    const wordStart = fuzzyMatch("cl", "meeting clip notes");
    const midWord = fuzzyMatch("cl", "declining offer");
    expect(wordStart).not.toBeNull();
    expect(midWord).not.toBeNull();
    expect(wordStart!.score).toBeGreaterThan(midWord!.score);
  });

  it("ranks an earlier match above the same match occurring later", () => {
    const early = fuzzyMatch("note", "note: ship before offsite, a note");
    const late = fuzzyMatch("note", "ship before offsite, a note");
    expect(early!.score).toBeGreaterThan(late!.score);
  });
});

describe("fuzzyFilter", () => {
  const items = ["Clipse landing copy", "declare a variable", "circle back tomorrow", "random unrelated text"];

  it("drops non-matches", () => {
    const hits = fuzzyFilter("zzz", items, (s) => s);
    expect(hits).toHaveLength(0);
  });

  it("returns every item, unscored, for an empty query", () => {
    const hits = fuzzyFilter("   ", items, (s) => s);
    expect(hits.map((h) => h.item)).toEqual(items);
    expect(hits.every((h) => h.score === 0)).toBe(true);
  });

  it("ranks best matches first", () => {
    const hits = fuzzyFilter("cl", items, (s) => s);
    expect(hits[0].item).toBe("Clipse landing copy");
    expect(hits.map((h) => h.item)).toContain("circle back tomorrow");
    expect(hits.map((h) => h.item)).not.toContain("random unrelated text");
  });

  it("is stable-ish: scores are non-increasing down the list", () => {
    const hits = fuzzyFilter("e", items, (s) => s);
    for (let i = 1; i < hits.length; i++) {
      expect(hits[i - 1].score).toBeGreaterThanOrEqual(hits[i].score);
    }
  });
});
