import { useEffect, useState } from "react";
import { api, isNotConnected, onConnectionChanged, onStatusChanged } from "../lib/tauri-client";
import type { DaemonStatus } from "../types/ipc";

/** How long a cold launch is given before an unreachable daemon is believed.
 * Long enough to cover opening a fresh database on a slow disk, short enough
 * that a genuinely dead daemon is still reported while someone is watching. */
const STARTUP_GRACE_MS = 12_000;
const RETRY_MS = 400;

export interface DaemonConnection {
  /** `null` until the first status reply or connection event arrives —
   * lets the UI show a neutral "connecting" state instead of guessing. */
  connected: boolean | null;
  status: DaemonStatus | null;
}

/** Tracks whether `clipsed` is reachable and the latest `DaemonStatus`,
 * across both the one-shot `status` command and the daemon's pushed
 * `status-changed` / `connection-changed` events. */
export function useDaemonConnection(): DaemonConnection {
  const [connected, setConnected] = useState<boolean | null>(null);
  const [status, setStatus] = useState<DaemonStatus | null>(null);

  useEffect(() => {
    let disposed = false;
    let retry: ReturnType<typeof setTimeout> | undefined;
    const startedAt = Date.now();

    /* The first `status` call almost always fails on a cold launch: the daemon
     * starts inside this process and has a database to open before it can
     * answer. Reporting that as "Clipse isn't running" makes a brand-new
     * install accuse itself of being broken in the first second of its life.
     *
     * So an unreachable daemon is only *believed* after it has had a fair
     * chance. Until then `connected` stays null and the window shows that it is
     * starting, which is the truth. */
    const ask = () => {
      api
        .status()
        .then((s) => {
          if (disposed) return;
          setStatus(s);
          setConnected(true);
        })
        .catch((err) => {
          if (disposed || !isNotConnected(err)) return;
          if (Date.now() - startedAt >= STARTUP_GRACE_MS) {
            setConnected(false);
            return;
          }
          retry = setTimeout(ask, RETRY_MS);
        });
    };
    ask();

    const unlistenStatus = onStatusChanged((s) => {
      if (disposed) return;
      setStatus(s);
      setConnected(true);
    });
    const unlistenConnection = onConnectionChanged((isConnected) => {
      if (disposed) return;
      setConnected(isConnected);
    });

    return () => {
      disposed = true;
      clearTimeout(retry);
      void unlistenStatus.then((fn) => fn());
      void unlistenConnection.then((fn) => fn());
    };
  }, []);

  return { connected, status };
}
