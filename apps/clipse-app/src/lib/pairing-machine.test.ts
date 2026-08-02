import { describe, expect, it } from "vitest";

import {
  type PairingState,
  canConnect,
  digitsOnly,
  isBusy,
  pairingReducer,
} from "./pairing-machine";

const code = { code: "482 913", expires_at_ms: 1_000 };
const paired = { peer_label: "laptop" };

function run(from: PairingState, ...actions: Parameters<typeof pairingReducer>[1][]) {
  return actions.reduce(pairingReducer, from);
}

describe("pairing machine", () => {
  it("walks the showing side from idle to paired", () => {
    const showing = run({ step: "idle" }, { type: "begin" }, { type: "showing", code });
    expect(showing).toEqual({ step: "showing", code });

    // The device showing the digits is told by an event, not by a click.
    const done = pairingReducer(showing, { type: "paired", paired });
    expect(done).toEqual({ step: "paired", peerLabel: "laptop" });
  });

  it("walks the typing side to the same place, with nothing in between", () => {
    const state = run({ step: "idle" }, { type: "connect" }, { type: "paired", paired });
    expect(state).toEqual({ step: "paired", peerLabel: "laptop" });
  });

  it("will not start a second ceremony while one is running", () => {
    expect(isBusy({ step: "idle" })).toBe(false);
    expect(isBusy({ step: "paired", peerLabel: "x" })).toBe(false);
    expect(isBusy({ step: "failed", reason: "x" })).toBe(false);
    expect(isBusy({ step: "showing", code })).toBe(true);
    expect(isBusy({ step: "connecting" })).toBe(true);
  });

  it("only offers to connect once six digits are actually typed", () => {
    expect(canConnect("48", { step: "idle" })).toBe(false);
    expect(canConnect("48291", { step: "idle" })).toBe(false);
    expect(canConnect("482 913", { step: "idle" })).toBe(true);
    expect(canConnect("482-913", { step: "idle" })).toBe(true);
    expect(canConnect("482913", { step: "connecting" })).toBe(
      false,
      // Asking the daemon twice is refused there anyway; not asking is better
      // than showing the user an error they did not cause.
    );
  });

  it("reads a code the way it is shown, and refuses anything else", () => {
    expect(digitsOnly("482 913")).toBe("482913");
    expect(digitsOnly("482-913")).toBe("482913");
    expect(digitsOnly("  4 8 2 9 1 3  ")).toBe("482913");
    expect(digitsOnly("48291399")).toBe("482913");
    expect(digitsOnly("abc")).toBe("");
  });

  it("cancelling from anywhere returns to idle", () => {
    for (const state of [
      { step: "showing", code } as PairingState,
      { step: "connecting" } as PairingState,
      { step: "failed", reason: "x" } as PairingState,
    ]) {
      expect(pairingReducer(state, { type: "cancel" })).toEqual({ step: "idle" });
    }
  });

  it("a failure is reported with its reason rather than swallowed", () => {
    const state = pairingReducer(
      { step: "connecting" },
      { type: "fail", reason: "no device on this network is showing that code" },
    );
    expect(state).toEqual({
      step: "failed",
      reason: "no device on this network is showing that code",
    });
  });
});
