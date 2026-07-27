import { useCallback, useEffect, useState } from "react";
import { api, isNotConnected } from "../lib/tauri-client";
import type { Settings } from "../types/ipc";

export interface SettingsState {
  settings: Settings | null;
  loading: boolean;
  saving: boolean;
  offline: boolean;
  errorMessage: string | null;
  /** Sends the full settings object to the daemon and applies the daemon's
   * (possibly normalized) response back into state. */
  save: (next: Settings) => Promise<void>;
}

export function useSettings(): SettingsState {
  const [settings, setSettings] = useState<Settings | null>(null);
  const [loading, setLoading] = useState(true);
  const [saving, setSaving] = useState(false);
  const [offline, setOffline] = useState(false);
  const [errorMessage, setErrorMessage] = useState<string | null>(null);

  useEffect(() => {
    let disposed = false;
    api
      .getSettings()
      .then((s) => {
        if (!disposed) setSettings(s);
      })
      .catch((err) => {
        if (disposed) return;
        if (isNotConnected(err)) setOffline(true);
        else setErrorMessage(err instanceof Error ? err.message : String(err));
      })
      .finally(() => {
        if (!disposed) setLoading(false);
      });
    return () => {
      disposed = true;
    };
  }, []);

  const save = useCallback(async (next: Settings) => {
    setSaving(true);
    setErrorMessage(null);
    try {
      const applied = await api.updateSettings(next);
      setSettings(applied);
      setOffline(false);
    } catch (err) {
      if (isNotConnected(err)) setOffline(true);
      else setErrorMessage(err instanceof Error ? err.message : String(err));
      throw err;
    } finally {
      setSaving(false);
    }
  }, []);

  return { settings, loading, saving, offline, errorMessage, save };
}
