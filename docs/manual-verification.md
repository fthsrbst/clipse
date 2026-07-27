# Manual verification

Green tests are not the bar. A phase is done when someone has driven the real
product and seen it behave. Record the date and platform when you run this.

`clipse-ctl` asks the daemon directly rather than reading a window, so a green
result means the daemon is right and not merely that the UI renders:

```bash
cargo run -p clipsed --example clipse-ctl -- --data-dir ./.clipse-dev/a status
```

## Log

**2026-07-27, Windows 11.** Daemon run against a scratch directory; five items
copied from another process; `clipse-ctl` queried afterwards. Three stored.
An AWS access key and a Luhn-valid card number were both suppressed; `order
12345678901234 shipped` was kept, which is the false-positive case the issuer
prefix rule exists for. Search, ordering and the source-device label were
correct. Items 4, 5, 7, 8, 9 and the GUI half of 1–3 below are still unrun.

## F1 — one device

Start a daemon against a scratch directory so nothing here touches a real
history:

```bash
cargo run -p clipsed -- --data-dir ./.clipse-dev/a --log debug
```

Then, in the app (`cd apps/clipse-app && pnpm tauri dev`):

1. **Capture.** Copy text in a browser, then in a terminal, then in an editor.
   All three appear in the history within a second, newest first, with the
   right source app where the platform reports one.
2. **Rich content.** Copy a styled paragraph from a word processor. It is *one*
   entry, not three, and pasting it into a rich editor keeps the formatting
   while pasting into a terminal gives plain text.
3. **Image.** Copy a screenshot. It appears with a preview, is stored as a
   blob (check `blobs/` has a new sharded file), and pastes back correctly.
4. **Password manager — the one that must not fail.** Copy a password from
   1Password/Bitwarden/KeePassXC. **Nothing appears in the history.** The
   daemon logs a suppression with a reason and no content. Check the database
   directly to be sure it is absent, not merely hidden by the UI.
5. **Secret detector.** Copy a JWT, an `sk_live_…` key and a card number.
   Suppressed. Then copy a git SHA, a UUID, a phone number and a long ordinary
   digit string — all four are kept. False positives cost real history.
6. **Dedup.** Copy the same text twice. One entry, moved to the top.
7. **Hotkey popup.** Ctrl/Cmd+Shift+V opens beside the cursor, on the monitor
   the cursor is on, fully inside the screen — including with the cursor at the
   far right edge and on a second monitor. Type to filter, arrow to move,
   Enter pastes into the previously focused window. Escape closes without
   pasting. Ctrl/Cmd+3 pastes the third item.
8. **Pin.** Pin an entry; it survives a daemon restart and stays pinned.
9. **Quota.** Set the blob quota low, copy several large images, and confirm
   old blobs are evicted while **every text entry survives** and pinned clips
   are untouched. The evicted entries stay in the list, marked incomplete.
10. **Daemon independence.** Quit the app. Copy something. Reopen the app — the
    clip is there. This is the whole reason the daemon is a separate process.
11. **Daemon absent.** Kill the daemon with the app open. The UI says so
    plainly and reconnects on its own when the daemon comes back.

## F2 — two devices

Run two daemons on one machine first, then repeat across real machines.

```bash
cargo run -p clipsed -- --data-dir ./.clipse-dev/a
cargo run -p clipsed -- --data-dir ./.clipse-dev/b
```

1. **Pairing.** The QR and six digits appear on A; entering them on B pairs.
   Both list each other. **Deliberately mistype the digits — pairing fails.**
2. **Sync.** Copy on A, paste on B within a second or two, and the source
   badge names A. Then the reverse.
3. **No loop.** Watch the logs while syncing: the clip crosses once. It must
   not ping-pong.
4. **Large content.** Copy a 20 MB image on A. It arrives on B, the transfer
   is chunked, and the entry is marked incomplete on B until it finishes.
5. **Resume.** Kill B mid-transfer. Restart it. The transfer resumes rather
   than restarting from zero.
6. **Offline.** Disconnect the network. Copy on both. Reconnect. Both
   histories converge, and both devices agree on the order.
7. **History-only mode.** Turn off "apply incoming to clipboard" on B. A clip
   from A lands in B's history but B's clipboard is untouched.
8. **Tailnet.** Put the two devices on different networks with Tailscale up.
   Sync still works. Then stop Tailscale — LAN-only still works at home.
9. **Removal.** Remove B from A. B can no longer decrypt anything from A, and
   A's remaining peers still sync.
10. **Privacy across devices.** Copy a password on A. It does not reach B.

## F3 — macOS notch

Build it first — nobody has:

```bash
cd apps/clipse-notch && swift build
```

On a notched MacBook: the panel hovers open, shows the last three clips, is
positioned from `NSScreen.safeAreaInsets` (verify on an external display too,
where there is no notch), accepts a drag-and-drop, and animates the source
device. It must not steal focus from the frontmost app.

## F4 — packaging

Install from the built artefact on a clean machine per platform. The installer
is signed (no SmartScreen warning on Windows, no Gatekeeper block on macOS),
the app launches, the daemon registers to start at login, and the updater
finds and applies a newer release.
