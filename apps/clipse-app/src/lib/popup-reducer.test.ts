import { describe, expect, it } from "vitest";
import { type PopupKeyState, popupKeyReducer } from "./popup-reducer";

function state(overrides: Partial<PopupKeyState> = {}): PopupKeyState {
  return { selectedIndex: 0, filter: "all", itemCount: 5, ...overrides };
}

describe("popupKeyReducer", () => {
  describe("arrow navigation", () => {
    it("moves down", () => {
      const { state: next } = popupKeyReducer(state({ selectedIndex: 1 }), { type: "ArrowDown" });
      expect(next.selectedIndex).toBe(2);
    });

    it("moves up", () => {
      const { state: next } = popupKeyReducer(state({ selectedIndex: 1 }), { type: "ArrowUp" });
      expect(next.selectedIndex).toBe(0);
    });

    it("wraps from the last item to the first on ArrowDown", () => {
      const { state: next } = popupKeyReducer(state({ selectedIndex: 4, itemCount: 5 }), { type: "ArrowDown" });
      expect(next.selectedIndex).toBe(0);
    });

    it("wraps from the first item to the last on ArrowUp", () => {
      const { state: next } = popupKeyReducer(state({ selectedIndex: 0, itemCount: 5 }), { type: "ArrowUp" });
      expect(next.selectedIndex).toBe(4);
    });

    it("is a no-op with an empty result list", () => {
      const { state: next, effect } = popupKeyReducer(state({ itemCount: 0 }), { type: "ArrowDown" });
      expect(next.selectedIndex).toBe(0);
      expect(effect).toEqual({ type: "none" });
    });
  });

  describe("Enter", () => {
    it("requests a paste of the selected index", () => {
      const { effect } = popupKeyReducer(state({ selectedIndex: 3 }), { type: "Enter" });
      expect(effect).toEqual({ type: "paste", index: 3 });
    });

    it("does nothing with an empty result list", () => {
      const { effect } = popupKeyReducer(state({ itemCount: 0 }), { type: "Enter" });
      expect(effect).toEqual({ type: "none" });
    });

    it("does not change selection state", () => {
      const s = state({ selectedIndex: 2 });
      const { state: next } = popupKeyReducer(s, { type: "Enter" });
      expect(next).toEqual(s);
    });
  });

  describe("Ctrl/Cmd+N (JumpTo)", () => {
    it("pastes the item at the given 0-based index", () => {
      const { effect, state: next } = popupKeyReducer(state({ itemCount: 9 }), { type: "JumpTo", index: 6 });
      expect(effect).toEqual({ type: "paste", index: 6 });
      expect(next.selectedIndex).toBe(6);
    });

    it("ignores an out-of-range index rather than pasting the wrong clip", () => {
      const { effect, state: next } = popupKeyReducer(state({ itemCount: 3, selectedIndex: 1 }), {
        type: "JumpTo",
        index: 8,
      });
      expect(effect).toEqual({ type: "none" });
      expect(next.selectedIndex).toBe(1);
    });

    it("ignores a negative index", () => {
      const { effect } = popupKeyReducer(state({ itemCount: 3 }), { type: "JumpTo", index: -1 });
      expect(effect).toEqual({ type: "none" });
    });
  });

  describe("Tab cycling", () => {
    it("cycles through filters in order and wraps back to all", () => {
      let s = state({ filter: "all" });
      const order = ["text", "image", "files", "link", "all"] as const;
      for (const expected of order) {
        s = popupKeyReducer(s, { type: "Tab" }).state;
        expect(s.filter).toBe(expected);
      }
    });

    it("resets selection to the top when the filter changes", () => {
      const { state: next } = popupKeyReducer(state({ filter: "all", selectedIndex: 4 }), { type: "Tab" });
      expect(next.selectedIndex).toBe(0);
    });
  });

  describe("Escape", () => {
    it("requests the popup close and leaves state untouched", () => {
      const s = state({ selectedIndex: 2, filter: "image" });
      const { state: next, effect } = popupKeyReducer(s, { type: "Escape" });
      expect(effect).toEqual({ type: "close" });
      expect(next).toEqual(s);
    });
  });

  describe("SetItemCount", () => {
    it("clamps the current selection when the list shrinks", () => {
      const { state: next } = popupKeyReducer(state({ selectedIndex: 4, itemCount: 5 }), {
        type: "SetItemCount",
        count: 2,
      });
      expect(next.selectedIndex).toBe(1);
      expect(next.itemCount).toBe(2);
    });

    it("resets to zero when the list becomes empty", () => {
      const { state: next } = popupKeyReducer(state({ selectedIndex: 4, itemCount: 5 }), {
        type: "SetItemCount",
        count: 0,
      });
      expect(next.selectedIndex).toBe(0);
    });

    it("leaves selection alone when it still fits", () => {
      const { state: next } = popupKeyReducer(state({ selectedIndex: 1, itemCount: 5 }), {
        type: "SetItemCount",
        count: 10,
      });
      expect(next.selectedIndex).toBe(1);
      expect(next.itemCount).toBe(10);
    });
  });
});
