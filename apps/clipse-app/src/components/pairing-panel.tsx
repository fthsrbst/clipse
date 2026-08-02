/**
 * Pairing, as the user experiences it.
 *
 * One number, one direction. The device you are standing at either shows six
 * digits or takes six digits — and once they are typed there is nothing left
 * to do: the two devices verify each other rather than asking the user to
 * compare anything. See `crates/clipse-crypto/src/pairing.rs` for why that is
 * safe, and for the one attack it does not stop.
 */

import { useEffect, useState } from "react";
import { useReducer } from "react";

import { DevicesIcon, CheckIcon } from "./icons";
import {
  type PairingState,
  canConnect,
  digitsOnly,
  isBusy,
  pairingReducer,
} from "../lib/pairing-machine";
import { api, onPairingEnded, onPairingSucceeded } from "../lib/tauri-client";
import styles from "./pairing-panel.module.css";

const INITIAL: PairingState = { step: "idle" };

export function PairingPanel({ onPaired }: { onPaired?: () => void }) {
  const [state, dispatch] = useReducer(pairingReducer, INITIAL);
  const [typed, setTyped] = useState("");

  // The device showing the digits finds out from an event: the ceremony
  // happens over the network, not through this window.
  useEffect(() => {
    const succeeded = onPairingSucceeded((peerLabel) => {
      dispatch({ type: "paired", paired: { peer_label: peerLabel } });
      onPaired?.();
    });
    const ended = onPairingEnded((reason) => dispatch({ type: "fail", reason }));
    return () => {
      void succeeded.then((un) => un());
      void ended.then((un) => un());
    };
  }, [onPaired]);

  async function show() {
    dispatch({ type: "begin" });
    try {
      dispatch({ type: "showing", code: await api.beginPairing() });
    } catch (err) {
      dispatch({ type: "fail", reason: message(err) });
    }
  }

  async function connect() {
    dispatch({ type: "connect" });
    try {
      const paired = await api.pairWithCode(digitsOnly(typed));
      dispatch({ type: "paired", paired });
      setTyped("");
      onPaired?.();
    } catch (err) {
      dispatch({ type: "fail", reason: message(err) });
    }
  }

  async function cancel() {
    try {
      await api.cancelPairing();
    } finally {
      dispatch({ type: "cancel" });
      setTyped("");
    }
  }

  return (
    <div className={styles.panel}>
      {state.step === "idle" && (
        <div className={styles.start}>
          <p className={styles.lead}>
            Pair another computer to share your clipboard with it. One of the
            two devices shows a six-digit code; you type it on the other.
          </p>
          <div className={styles.row}>
            <button type="button" className={styles.primary} onClick={show}>
              <DevicesIcon /> Show a code
            </button>
          </div>
          <div className={styles.divider}>or type the code from the other device</div>
          <div className={styles.row}>
            <input
              className={styles.codeInput}
              placeholder="000000"
              inputMode="numeric"
              autoComplete="off"
              maxLength={7}
              value={typed}
              onChange={(e) => setTyped(digitsOnly(e.target.value))}
              onKeyDown={(e) => {
                if (e.key === "Enter" && canConnect(typed, state)) void connect();
              }}
              aria-label="The six digits shown on the other device"
            />
            <button
              type="button"
              className={styles.secondary}
              disabled={!canConnect(typed, state)}
              onClick={connect}
            >
              Connect
            </button>
          </div>
        </div>
      )}

      {state.step === "starting" && <p className={styles.lead}>Preparing…</p>}
      {state.step === "connecting" && (
        <p className={styles.lead}>Looking for the device showing that code…</p>
      )}

      {state.step === "showing" && (
        <div className={styles.start}>
          <p className={styles.lead}>
            Type these six digits into Clipse on the other computer. They stop
            working in a few minutes.
          </p>
          <div className={styles.digits} aria-label="Pairing code">
            {state.code.code}
          </div>
          <p className={styles.warning}>
            Only ever read this code off your own screen. Anyone you send it to
            can pair with this device while it is showing.
          </p>
          <div className={styles.row}>
            <button type="button" className={styles.ghost} onClick={cancel}>
              Cancel
            </button>
          </div>
        </div>
      )}

      {state.step === "paired" && (
        <div className={styles.start}>
          <p className={styles.done}>
            <CheckIcon /> Paired with {state.peerLabel || "the other device"}.
            Your clipboard is syncing.
          </p>
          <button
            type="button"
            className={styles.ghost}
            onClick={() => dispatch({ type: "cancel" })}
          >
            Pair another
          </button>
        </div>
      )}

      {state.step === "failed" && (
        <div className={styles.start}>
          <p className={styles.error}>{state.reason}</p>
          <button
            type="button"
            className={styles.secondary}
            onClick={() => dispatch({ type: "cancel" })}
          >
            Try again
          </button>
        </div>
      )}

      {isBusy(state) && state.step !== "showing" && (
        <button type="button" className={styles.ghost} onClick={cancel}>
          Cancel
        </button>
      )}
    </div>
  );
}

function message(err: unknown): string {
  return err instanceof Error ? err.message : String(err);
}
