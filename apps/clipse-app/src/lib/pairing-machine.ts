/**
 * What the pairing screen is doing, as a value.
 *
 * Kept out of the component because the one thing that must not go wrong here
 * is confirming a pairing the user never actually looked at. As a reducer it
 * can be tested exhaustively: there is no path from any state to `confirming`
 * that does not pass through `comparing`, where both codes are on screen.
 */

import type { PairingCode, PairingOffer } from "../types/ipc";

export type PairingState =
  | { step: "idle" }
  /** Asked the daemon for an offer, waiting for it. */
  | { step: "starting" }
  /** Showing our offer; nobody has answered yet. */
  | { step: "offering"; offer: PairingOffer }
  /** Answering someone else's offer, waiting on the network. */
  | { step: "answering" }
  /** Both codes exist. This is the only state confirmation may follow. */
  | { step: "comparing"; code: PairingCode; role: "offered" | "answered" }
  | { step: "confirming" }
  | { step: "paired"; peerLabel: string }
  | { step: "failed"; reason: string };

export type PairingAction =
  | { type: "begin" }
  | { type: "offered"; offer: PairingOffer }
  | { type: "answer" }
  /** From `pair_with_uri` on this device, or the `pairing-code` event on the
   * device that showed the offer. */
  | { type: "code"; code: PairingCode; role: "offered" | "answered" }
  | { type: "confirm" }
  | { type: "confirmed" }
  | { type: "cancel" }
  | { type: "fail"; reason: string };

export function pairingReducer(state: PairingState, action: PairingAction): PairingState {
  switch (action.type) {
    case "begin":
      return { step: "starting" };
    case "offered":
      return { step: "offering", offer: action.offer };
    case "answer":
      return { step: "answering" };
    case "code":
      return { step: "comparing", code: action.code, role: action.role };

    case "confirm":
      // The guard that matters. Confirming from anywhere else would mean
      // trusting a device whose code was never on screen.
      return state.step === "comparing" ? { step: "confirming" } : state;

    case "confirmed":
      return state.step === "confirming"
        ? { step: "paired", peerLabel: "" }
        : state;

    case "cancel":
      return { step: "idle" };
    case "fail":
      return { step: "failed", reason: action.reason };
  }
}

/** May the "these match" button be pressed right now? */
export function canConfirm(state: PairingState): boolean {
  return state.step === "comparing";
}

/** Is a pairing attempt occupying the daemon, so a second must not start? */
export function isBusy(state: PairingState): boolean {
  return (
    state.step === "starting" ||
    state.step === "offering" ||
    state.step === "answering" ||
    state.step === "comparing" ||
    state.step === "confirming"
  );
}
