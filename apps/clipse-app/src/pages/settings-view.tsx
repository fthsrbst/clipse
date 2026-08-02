import { useEffect, useState } from "react";
import { HotkeyInput } from "../components/hotkey-input";
import { ToggleSwitch } from "../components/toggle-switch";
import { TagListInput } from "../components/tag-list-input";
import { PairingPanel } from "../components/pairing-panel";
import { DeviceList } from "../components/device-list";
import { CaptureModeBanner } from "../components/capture-mode-banner";
import { DaemonOfflineState } from "../components/daemon-offline-state";
import { WindowControls } from "../components/window-controls";
import { AsciiLogo } from "../components/ascii-logo";
import { EmptyState } from "../components/empty-state";
import { useSettings } from "../hooks/use-settings";
import type { DaemonStatus, Settings } from "../types/ipc";
import styles from "./settings-view.module.css";

const GB = 1024 * 1024 * 1024;

export interface SettingsViewProps {
  status: DaemonStatus | null;
}

/** The second view of the history window, not a screen of its own: the spine
 * stays mounted beside it and owns the navigation, so there is no back button
 * here to draw. Escape returns — see `history-window.tsx`. */
export function SettingsView({ status }: SettingsViewProps) {
  const { settings, loading, saving, offline, errorMessage, save } = useSettings();
  const [draft, setDraft] = useState<Settings | null>(null);
  const [savedFlash, setSavedFlash] = useState(false);

  useEffect(() => {
    if (settings) setDraft(settings);
  }, [settings]);

  const dirty = draft !== null && settings !== null && JSON.stringify(draft) !== JSON.stringify(settings);

  function patch(partial: Partial<Settings>) {
    setDraft((prev) => (prev ? { ...prev, ...partial } : prev));
  }

  async function handleSave() {
    if (!draft) return;
    try {
      await save(draft);
      setSavedFlash(true);
      setTimeout(() => setSavedFlash(false), 1600);
    } catch {
      // Surfaced via errorMessage / offline below.
    }
  }

  return (
    <div className={styles.window}>
      <header className={styles.header} data-tauri-drag-region>
        <span className={styles.title}>Settings</span>
        <span className={styles.hint}>esc to close</span>
        <WindowControls />
      </header>

      <div className={styles.body}>
        {status && status.capture_mode !== "Automatic" && (
          <div className={styles.section}>
            <CaptureModeBanner captureMode={status.capture_mode} />
          </div>
        )}

        {offline ? (
          <DaemonOfflineState />
        ) : loading ? (
          <EmptyState title="Loading settings…" animated />
        ) : !draft ? (
          <EmptyState
            title="Couldn't load settings"
            description={errorMessage ?? "Something went wrong talking to Clipse."}
          />
        ) : (
          <>
            <Section index="01" title="Capture">
              <Row label="Hotkey" description="Opens the quick-paste popup from anywhere.">
                <HotkeyInput value={draft.hotkey} onChange={(hotkey) => patch({ hotkey })} />
              </Row>
              <Row
                label="Apply incoming clips to clipboard"
                description="When off, clips synced from other devices land in history but don't take over your clipboard."
              >
                <ToggleSwitch
                  checked={draft.apply_incoming_to_clipboard}
                  onChange={(v) => patch({ apply_incoming_to_clipboard: v })}
                  label="Apply incoming clips to clipboard"
                />
              </Row>
              <Row label="Detect secrets" description="Skip capturing content that looks like a password or API key.">
                <ToggleSwitch
                  checked={draft.detect_secrets}
                  onChange={(v) => patch({ detect_secrets: v })}
                  label="Detect secrets"
                />
              </Row>
              <Row label="Start at login" description="Launch Clipse automatically when you sign in.">
                <ToggleSwitch
                  checked={draft.start_at_login}
                  onChange={(v) => patch({ start_at_login: v })}
                  label="Start at login"
                />
              </Row>
            </Section>

            <Section index="02" title="This device">
              <Row label="Device label" description="How this device appears to your other paired devices.">
                <input
                  className={styles.textInput}
                  type="text"
                  value={draft.device_label}
                  onChange={(e) => patch({ device_label: e.target.value })}
                />
              </Row>
              {/* Presence, not access. Worth its own row because the thing it
               * controls is visible to strangers and nothing else in the app
               * would ever tell you so. */}
              <Row
                label="Announce on this network"
                description="Lets your paired devices find this one again after its address changes. Anyone else on the network can see that this machine runs Clipse, under the label above — they still cannot see or ask for your clipboard. Turn it off to stay quiet; paired devices are then reached at the addresses recorded when you paired."
              >
                <ToggleSwitch
                  checked={draft.announce_on_network}
                  onChange={(v) => patch({ announce_on_network: v })}
                  label="Announce on this network"
                />
              </Row>
              <Row
                label="Blob storage quota"
                description="Large payloads (screenshots, big files) beyond this size evict the oldest unpinned blob first."
              >
                <div className={styles.quotaRow}>
                  <input
                    className={styles.numberInput}
                    type="number"
                    min={0.1}
                    step={0.5}
                    value={Math.round((draft.blob_quota_bytes / GB) * 10) / 10}
                    onChange={(e) => {
                      const gb = Number(e.target.value);
                      if (Number.isFinite(gb) && gb > 0) patch({ blob_quota_bytes: Math.round(gb * GB) });
                    }}
                  />
                  <span className={styles.unit}>GB</span>
                </div>
              </Row>
            </Section>

            <Section index="03" title="Never captured">
              <Row label="Blocked apps" description="Clipboard activity from these apps is never captured." stacked>
                <TagListInput values={draft.blocked_apps} onChange={(blocked_apps) => patch({ blocked_apps })} />
              </Row>
            </Section>

            <Section index="04" title="Your devices">
              <Row
                label="Paired devices"
                description="Your clipboard is only ever shared with devices you pair here."
                stacked
              >
                <DeviceList />
                <PairingPanel />
              </Row>
            </Section>

            {/* The colophon. A tool that watches everything you copy should be
             * easy to go and read the source of, so the repository is one click
             * away rather than buried in a README nobody opens. */}
            <section className={styles.section}>
              <div className={styles.colophon}>
                <div>
                  <AsciiLogo variant="wordmark" cell={6.5} className={styles.colophonMark} />
                  <p className={styles.colophonLine}>
                    Free and open source, built by Fatih Serbest.
                  </p>
                </div>
                <a
                  className={styles.colophonLink}
                  href="https://github.com/fthsrbst/clipse"
                  target="_blank"
                  rel="noreferrer noopener"
                >
                  github.com/fthsrbst/clipse ↗
                </a>
              </div>
            </section>

            <div className={styles.saveBar}>
              {errorMessage && <span className={styles.error}>{errorMessage}</span>}
              {savedFlash && <span className={styles.saved}>Saved</span>}
              <button type="button" className={styles.saveButton} disabled={!dirty || saving} onClick={() => void handleSave()}>
                {saving ? "Saving…" : "Save changes"}
              </button>
            </div>
          </>
        )}
      </div>
    </div>
  );
}

/** A numbered band, the way the onboarding numbers its steps. The numeral sits
 * in the margin and the rule runs out to the edge, so the heading reads as a
 * mark on the page rather than as the lid of a box. */
function Section({
  index,
  title,
  children,
}: {
  index: string;
  title: string;
  children: React.ReactNode;
}) {
  return (
    <section className={styles.section}>
      <div className={styles.sectionHead}>
        <span className={styles.index} data-numeric>
          {index}
        </span>
        <h2 className={styles.sectionTitle}>{title}</h2>
        <span className={styles.sectionRule} aria-hidden="true" />
      </div>
      {children}
    </section>
  );
}

function Row({
  label,
  description,
  children,
  stacked = false,
}: {
  label: string;
  description?: string;
  children: React.ReactNode;
  stacked?: boolean;
}) {
  return (
    <div className={stacked ? `${styles.row} ${styles.stacked}` : styles.row}>
      <div className={styles.rowText}>
        <p className={styles.rowLabel}>{label}</p>
        {description && <p className={styles.rowDescription}>{description}</p>}
      </div>
      <div className={styles.rowControl}>{children}</div>
    </div>
  );
}
