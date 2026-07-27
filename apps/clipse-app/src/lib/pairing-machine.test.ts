import { describe, expect, it } from "vitest";

import {
  type PairingState,
  canConfirm,
  isBusy,
  pairingReducer,
} from "./pairing-machine";

const offer = { uri: "clipse://pair/abc", expires_at_ms: 1_000, svg: null };
const code = { digits: "123 456", peer_label: "laptop" };

function run(from: PairingState, ...actions: Parameters<typeof pairingReducer>[1][]) {
  return actions.reduce(pairingReducer, from);
}

describe("pairing machine", () => {
  it("walks the offering side from idle to a comparison", () => {
    const state = run({ step: "idle" }, { type: "begin" }, { type: "offered", offer }, {
      type: "code",
      code,
      role: "offered",
    });
    expect(state).toEqual({ step: "comparing", code, role: "offered" });
    expect(canConfirm(state)).toBe(true);
  });

  it("walks the answering side to the same place", () => {
    const state = run({ step: "idle" }, { type: "answer" }, {
      type: "code",
      code,
      role: "answered",
    });
    expect(state.step).toBe("comparing");
  });

  // The whole reason this is a reducer.
  it("refuses to confirm from any state where no code is on screen", () => {
    const withoutCode: PairingState[] = [
      { step: "idle" },
      { step: "starting" },
      { step: "offering", offer },
      { step: "answering" },
      { step: "confirming" },
      { step: "paired", peerLabel: "laptop" },
      { step: "failed", reason: "nope" },
    ];

    for (const state of withoutCode) {
      expect(canConfirm(state)).toBe(false);
      expect(pairingReducer(state, { type: "confirm" })).toEqual(state);
    }
  });

  it("only reaches paired by way of confirming", () => {
    expect(pairingReducer({ step: "comparing", code, role: "offered" }, { type: "confirmed" }))
      .toEqual({ step: "comparing", code, role: "offered" });

    const proper = run({ step: "comparing", code, role: "offered" }, { type: "confirm" }, {
      type: "confirmed",
    });
    expect(proper.step).toBe("paired");
  });

  it("cancelling from anywhere returns to idle", () => {
    for (const state of [
      { step: "offering", offer } as PairingState,
      { step: "comparing", code, role: "offered" } as PairingState,
      { step: "failed", reason: "x" } as PairingState,
    ]) {
      expect(pairingReducer(state, { type: "cancel" })).toEqual({ step: "idle" });
    }
  });

  it("knows when the daemon is already busy with a ceremony", () => {
    expect(isBusy({ step: "idle" })).toBe(false);
    expect(isBusy({ step: "paired", peerLabel: "x" })).toBe(false);
    expect(isBusy({ step: "failed", reason: "x" })).toBe(false);
    expect(isBusy({ step: "offering", offer })).toBe(true);
    expect(isBusy({ step: "comparing", code, role: "offered" })).toBe(true);
  });

  it("a failure is reported with its reason rather than swallowed", () => {
    const state = pairingReducer({ step: "answering" }, {
      type: "fail",
      reason: "that code has expired",
    });
    expect(state).toEqual({ step: "failed", reason: "that code has expired" });
  });
});
