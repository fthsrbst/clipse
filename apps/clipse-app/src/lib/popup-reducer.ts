/**
 * Pure keyboard state machine for the hotkey popup.
 *
 * Kept free of `invoke()` calls on purpose: the reducer only ever decides
 * *what should happen* (an effect the caller must carry out) and returns the
 * next selection/filter state. That makes arrow-wrap, Ctrl+N, Tab cycling
 * and Enter/Escape testable as plain data in/data out, with the actual
 * `paste`/`hide_popup` calls left to the component driving it.
 */

export const CLIP_TYPE_FILTERS = ["all", "text", "image", "files", "link"] as const;

export type ClipTypeFilter = (typeof CLIP_TYPE_FILTERS)[number];

export interface PopupKeyState {
  /** Index into the currently visible (filtered) result list. */
  selectedIndex: number;
  filter: ClipTypeFilter;
  /** Size of the currently visible result list — the reducer needs this to
   * wrap arrow navigation and to validate Ctrl+N jumps and Enter. */
  itemCount: number;
}

export type PopupKeyAction =
  | { type: "ArrowDown" }
  | { type: "ArrowUp" }
  | { type: "Tab" }
  | { type: "Enter" }
  /** Ctrl/Cmd+1..9 — `index` is 0-based (Ctrl+1 -> index 0). */
  | { type: "JumpTo"; index: number }
  | { type: "Escape" }
  /** Fired whenever the visible result count changes (new query, new
   * filter, a live clip-added/removed event) so selection stays in range. */
  | { type: "SetItemCount"; count: number };

export type PopupKeyEffect =
  | { type: "none" }
  | { type: "paste"; index: number }
  | { type: "close" };

export interface PopupKeyResult {
  state: PopupKeyState;
  effect: PopupKeyEffect;
}

const NONE: PopupKeyEffect = { type: "none" };

export function initialPopupKeyState(): PopupKeyState {
  return { selectedIndex: 0, filter: "all", itemCount: 0 };
}

function clampIndex(index: number, itemCount: number): number {
  if (itemCount <= 0) return 0;
  return Math.min(Math.max(index, 0), itemCount - 1);
}

export function popupKeyReducer(state: PopupKeyState, action: PopupKeyAction): PopupKeyResult {
  switch (action.type) {
    case "ArrowDown": {
      if (state.itemCount === 0) return { state, effect: NONE };
      const next = (state.selectedIndex + 1) % state.itemCount;
      return { state: { ...state, selectedIndex: next }, effect: NONE };
    }

    case "ArrowUp": {
      if (state.itemCount === 0) return { state, effect: NONE };
      const next = (state.selectedIndex - 1 + state.itemCount) % state.itemCount;
      return { state: { ...state, selectedIndex: next }, effect: NONE };
    }

    case "Tab": {
      const currentIndex = CLIP_TYPE_FILTERS.indexOf(state.filter);
      const nextFilter = CLIP_TYPE_FILTERS[(currentIndex + 1) % CLIP_TYPE_FILTERS.length];
      // The result list is about to change under a new filter — start back
      // at the top rather than pointing at whatever row happens to land on
      // the old selectedIndex.
      return { state: { ...state, filter: nextFilter, selectedIndex: 0 }, effect: NONE };
    }

    case "Enter": {
      if (state.itemCount === 0) return { state, effect: NONE };
      return { state, effect: { type: "paste", index: state.selectedIndex } };
    }

    case "JumpTo": {
      if (action.index < 0 || action.index >= state.itemCount) {
        return { state, effect: NONE };
      }
      return {
        state: { ...state, selectedIndex: action.index },
        effect: { type: "paste", index: action.index },
      };
    }

    case "Escape":
      return { state, effect: { type: "close" } };

    case "SetItemCount": {
      const count = Math.max(0, action.count);
      return { state: { ...state, itemCount: count, selectedIndex: clampIndex(state.selectedIndex, count) }, effect: NONE };
    }

    default:
      return { state, effect: NONE };
  }
}
