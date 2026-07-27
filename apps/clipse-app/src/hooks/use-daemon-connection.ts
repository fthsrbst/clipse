import { useEffect, useState } from "react";
import { api, isNotConnected, onConnectionChanged, onStatusChanged } from "../lib/tauri-client";
import type { DaemonStatus } from "../types/ipc";

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

    api
      .status()
      .then((s) => {
        if (disposed) return;
        setStatus(s);
        setConnected(true);
      })
      .catch((err) => {
        if (disposed || !isNotConnected(err)) return;
        setConnected(false);
      });

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
      void unlistenStatus.then((fn) => fn());
      void unlistenConnection.then((fn) => fn());
    };
  }, []);

  return { connected, status };
}
