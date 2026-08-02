/**
 * What the pairing screen is doing, as a value.
 *
 * Kept out of the component because the screen has two halves that must never
 * run at once: showing a code and typing one. A device that is doing both is
 * offering a ceremony to itself, and the daemon refuses the second one
 * anyway — the reducer is what stops the UI from asking.
 *
 * There is no "compare two codes" state any more. The devices verify each
 * other; see `crates/clipse-crypto/src/pairing.rs`.
 */

import type { Paired, PairingCode } from "../types/ipc";

export type PairingState =
  | { step: "idle" }
  /** Asked the daemon for a code, waiting for it. */
  | { step: "starting" }
  /** Showing our six digits; nobody has typed them yet. */
  | { step: "showing"; code: PairingCode }
  /** The user typed six digits and the daemon is looking for that device. */
  | { step: "connecting" }
  | { step: "paired"; peerLabel: string }
  | { step: "failed"; reason: string };

export type PairingAction =
  | { type: "begin" }
  | { type: "showing"; code: PairingCode }
  | { type: "connect" }
  /** From `pair_with_code` on this device, or the `pairing-succeeded` event on
   * the device that was showing the digits. */
  | { type: "paired"; paired: Paired }
  | { type: "cancel" }
  | { type: "fail"; reason: string };

export function pairingReducer(_state: PairingState, action: PairingAction): PairingState {
  switch (action.type) {
    case "begin":
      return { step: "starting" };
    case "showing":
      return { step: "showing", code: action.code };
    case "connect":
      return { step: "connecting" };
    case "paired":
      return { step: "paired", peerLabel: action.paired.peer_label };
    case "cancel":
      return { step: "idle" };
    case "fail":
      // A failure while showing a code is about *that* ceremony, not about the
      // screen: the daemon has dropped the offer, so the code on screen is
      // dead and saying so is the only honest thing left.
      return { step: "failed", reason: action.reason };
  }
}

/** Is a pairing attempt occupying the daemon, so a second must not start? */
export function isBusy(state: PairingState): boolean {
  return state.step === "starting" || state.step === "showing" || state.step === "connecting";
}

/** Six digits, with the spaces and dashes people type left out. */
export function digitsOnly(typed: string): string {
  return typed.replace(/\D/g, "").slice(0, 6);
}

/** May "Connect" be pressed? Only with six digits in hand, so the daemon is
 * never asked to walk the network for a half-typed code. */
export function canConnect(typed: string, state: PairingState): boolean {
  return digitsOnly(typed).length === 6 && !isBusy(state);
}
