/**
 * Pairing, as the user experiences it.
 *
 * The screen's job is not to display two codes — it is to *ask* whether they
 * match. Everything else here (the offer string, the paste field, the states)
 * exists to get to that question honestly, and the confirm button is disabled
 * until both codes are actually on screen.
 *
 * The offer is shown as a copyable string rather than a QR image: drawing a
 * real QR code means writing an encoder, and this app takes no dependency it
 * does not need. Copying the string between devices pairs them exactly the
 * same way.
 */

import { useEffect, useReducer, useState } from "react";

import { DevicesIcon, CheckIcon, CloseIcon } from "./icons";
import {
  type PairingState,
  canConfirm,
  isBusy,
  pairingReducer,
} from "../lib/pairing-machine";
import { api, onPairingCode, onPairingEnded } from "../lib/tauri-client";
import styles from "./pairing-panel.module.css";

const INITIAL: PairingState = { step: "idle" };

export function PairingPanel({ onPaired }: { onPaired?: () => void }) {
  const [state, dispatch] = useReducer(pairingReducer, INITIAL);
  const [pasted, setPasted] = useState("");
  const [copied, setCopied] = useState(false);

  // The device that showed the offer learns its code from an event, because
  // the answer arrives over the network rather than through this window.
  useEffect(() => {
    const code = onPairingCode((code) => dispatch({ type: "code", code, role: "offered" }));
    const ended = onPairingEnded((reason) => dispatch({ type: "fail", reason }));
    return () => {
      void code.then((un) => un());
      void ended.then((un) => un());
    };
  }, []);

  async function begin() {
    dispatch({ type: "begin" });
    try {
      dispatch({ type: "offered", offer: await api.beginPairing() });
    } catch (err) {
      dispatch({ type: "fail", reason: message(err) });
    }
  }

  async function answer() {
    dispatch({ type: "answer" });
    try {
      const code = await api.pairWithUri(pasted.trim());
      dispatch({ type: "code", code, role: "answered" });
    } catch (err) {
      dispatch({ type: "fail", reason: message(err) });
    }
  }

  async function decide(matches: boolean) {
    if (matches) dispatch({ type: "confirm" });
    try {
      await api.confirmPairing(matches);
      if (matches) {
        dispatch({ type: "confirmed" });
        onPaired?.();
      } else {
        dispatch({ type: "cancel" });
      }
    } catch (err) {
      dispatch({ type: "fail", reason: message(err) });
    }
  }

  async function cancel() {
    try {
      await api.cancelPairing();
    } finally {
      dispatch({ type: "cancel" });
      setPasted("");
    }
  }

  return (
    <div className={styles.panel}>
      {state.step === "idle" && (
        <div className={styles.start}>
          <p className={styles.lead}>
            Pair another computer to share your clipboard with it. Nothing
            leaves this device until you have compared a code on both screens.
          </p>
          <div className={styles.row}>
            <button type="button" className={styles.primary} onClick={begin}>
              <DevicesIcon /> Show a pairing code
            </button>
          </div>
          <div className={styles.divider}>or paste one from the other device</div>
          <div className={styles.row}>
            <input
              className={styles.input}
              placeholder="clipse://pair/…"
              value={pasted}
              onChange={(e) => setPasted(e.target.value)}
              aria-label="Pairing code from the other device"
            />
            <button
              type="button"
              className={styles.secondary}
              disabled={!pasted.trim().startsWith("clipse://pair/")}
              onClick={answer}
            >
              Connect
            </button>
          </div>
        </div>
      )}

      {state.step === "starting" && <p className={styles.lead}>Preparing…</p>}
      {state.step === "answering" && <p className={styles.lead}>Reaching the other device…</p>}

      {state.step === "offering" && (
        <div className={styles.start}>
          <p className={styles.lead}>
            Paste this into Clipse on the other computer. It stops working in a
            few minutes.
          </p>
          <code className={styles.offer}>{state.offer.uri}</code>
          <div className={styles.row}>
            <button
              type="button"
              className={styles.secondary}
              onClick={async () => {
                await navigator.clipboard.writeText(state.offer.uri);
                setCopied(true);
              }}
            >
              {copied ? <CheckIcon /> : null} {copied ? "Copied" : "Copy"}
            </button>
            <button type="button" className={styles.ghost} onClick={cancel}>
              Cancel
            </button>
          </div>
        </div>
      )}

      {state.step === "comparing" && (
        <div className={styles.compare}>
          <p className={styles.lead}>
            <strong>{state.code.peer_label}</strong> answered. Does it show
            these same six digits?
          </p>
          <div className={styles.digits} aria-label="Pairing code">
            {state.code.digits}
          </div>
          <p className={styles.warning}>
            If the two screens differ, someone is between your devices. Say no.
          </p>
          <div className={styles.row}>
            <button
              type="button"
              className={styles.primary}
              disabled={!canConfirm(state)}
              onClick={() => decide(true)}
            >
              <CheckIcon /> They match
            </button>
            <button type="button" className={styles.danger} onClick={() => decide(false)}>
              <CloseIcon /> They do not
            </button>
          </div>
        </div>
      )}

      {state.step === "confirming" && <p className={styles.lead}>Pairing…</p>}

      {state.step === "paired" && (
        <div className={styles.start}>
          <p className={styles.done}>
            <CheckIcon /> Paired. Your clipboard will start syncing shortly.
          </p>
          <button type="button" className={styles.ghost} onClick={() => dispatch({ type: "cancel" })}>
            Pair another
          </button>
        </div>
      )}

      {state.step === "failed" && (
        <div className={styles.start}>
          <p className={styles.error}>{state.reason}</p>
          <button type="button" className={styles.secondary} onClick={() => dispatch({ type: "cancel" })}>
            Try again
          </button>
        </div>
      )}

      {isBusy(state) && state.step !== "comparing" && state.step !== "offering" && (
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
