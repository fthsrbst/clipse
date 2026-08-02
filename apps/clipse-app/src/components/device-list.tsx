/**
 * The devices this one is paired with, and whether they are actually
 * reachable.
 *
 * "Paired" and "reachable" are two different facts and the list says both:
 * a device that paired last week and has not answered since is still yours,
 * but pretending it is online would make the whole sync status meaningless.
 */

import { useCallback, useEffect, useState } from "react";

import { DevicesIcon } from "./icons";
import { api, onDeviceChanged, onDevicesChanged } from "../lib/tauri-client";
import { formatRelativeTime } from "../lib/relative-time";
import type { PeerInfo } from "../types/ipc";
import styles from "./device-list.module.css";

export function DeviceList() {
  const [devices, setDevices] = useState<PeerInfo[] | null>(null);
  const [busy, setBusy] = useState<string | null>(null);

  const refresh = useCallback(async () => {
    try {
      setDevices(await api.devices());
    } catch {
      // The daemon being unreachable is already shown by the offline state
      // above this panel; an empty list here would be a second, wrong story.
    }
  }, []);

  useEffect(() => {
    void refresh();

    const all = onDevicesChanged((peers) => setDevices(peers));
    const one = onDeviceChanged((peer) =>
      setDevices((current) => {
        if (!current) return [peer];
        const known = current.some((device) => device.device === peer.device);
        return known
          ? current.map((device) => (device.device === peer.device ? peer : device))
          : [...current, peer];
      }),
    );

    return () => {
      void all.then((un) => un());
      void one.then((un) => un());
    };
  }, [refresh]);

  async function forget(device: PeerInfo) {
    setBusy(device.device);
    try {
      await api.forgetDevice(device.device);
      setDevices((current) => current?.filter((d) => d.device !== device.device) ?? null);
    } catch {
      // Left in the list on purpose: it is still paired if the daemon said no.
      void refresh();
    } finally {
      setBusy(null);
    }
  }

  if (devices === null) return null;
  if (devices.length === 0) {
    return (
      <p className={styles.empty}>
        No devices paired yet. Pair one below and your clipboard follows you
        between them.
      </p>
    );
  }

  return (
    <ul className={styles.list}>
      {devices.map((device) => (
        <li key={device.device} className={styles.row}>
          <DevicesIcon />
          <div className={styles.identity}>
            <span className={styles.label}>{device.label}</span>
            <span className={styles.where}>{describe(device)}</span>
          </div>
          <button
            type="button"
            className={styles.forget}
            disabled={busy === device.device}
            onClick={() => void forget(device)}
          >
            Forget
          </button>
        </li>
      ))}
    </ul>
  );
}

function describe(device: PeerInfo): string {
  const platform = platformName(device.platform);
  switch (device.connectivity) {
    case "Lan":
      return `${platform} · on this network`;
    case "Tailnet":
      return `${platform} · over Tailscale`;
    case "Offline":
      return device.last_seen_ms === null
        ? `${platform} · not reached yet`
        : `${platform} · last synced ${formatRelativeTime(device.last_seen_ms)}`;
  }
}

function platformName(platform: string): string {
  switch (platform) {
    case "macos":
      return "macOS";
    case "windows":
      return "Windows";
    case "linux":
      return "Linux";
    default:
      return platform;
  }
}
