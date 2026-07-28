# Manual verification

Green tests are not the bar. A phase is done when someone has driven the real
product and seen it behave. Record the date and platform when you run this.

`clipse-ctl` asks the daemon directly rather than reading a window, so a green
result means the daemon is right and not merely that the UI renders:

```bash
cargo run -p clipsed --example clipse-ctl -- --data-dir ./.clipse-dev/a status
```

## Two ways to waste an afternoon

**Do not run `target/debug/clipse-app.exe` directly.** In a debug profile Tauri
serves the frontend from `devUrl`, not from the bundled `frontendDist`, so the
window opens on `ERR_CONNECTION_REFUSED` and looks like a broken app. Use
`pnpm tauri dev`, or build a release profile, which embeds `dist`.

**Point the app and the daemon at the data directory the same way.** `tauri dev`
runs the binary from `apps/clipse-app/src-tauri`, so a relative `CLIPSE_DATA_DIR`
does not mean what it means to a daemon started from the repo root — you get a
daemon and a UI in two different directories, and the UI can only say "Clipse
isn't running". Pass an absolute path to both.

## Log

**2026-07-27, Windows 11.** Daemon run against a scratch directory; five items
copied from another process; `clipse-ctl` queried afterwards. Three stored.
An AWS access key and a Luhn-valid card number were both suppressed; `order
12345678901234 shipped` was kept, which is the false-positive case the issuer
prefix rule exists for. Search, ordering and the source-device label were
correct. Items 4, 5, 7, 8, 9 and the GUI half of 1–3 below are still unrun.

**2026-07-28, Windows 11 — the GUI, seen at last.** `pnpm tauri dev` against a
running daemon. The history window renders: previews, relative timestamps, the
source-device label, a link glyph on the URL, per-row pin/copy/delete, and the
footer count. Images captured from another process arrived with their size
(75 KB and 1.3 MB), so the blob path is exercised too. Killing the daemon under
a live window shows the "Clipse isn't running" state, and restarting it
reconnects on its own — no reload, no button. Suppressed items return nothing
from search, which is the stronger claim: they are absent from the store rather
than hidden by the UI. Still unrun here: the pairing screen and the hotkey
popup, both of which need a driven UI.

**2026-07-28, macOS 26.5.2 (arm64).** First execution of the macOS clipboard
backend — until today it had only ever been type-checked. Capture, previews,
formats and sizes correct. Both secrets suppressed, confirmed by `strings` over
`clipse.db`, `-wal` and `-shm` rather than by asking the daemon. `SIGTERM` exits
in about a second with no lingering process, so the Windows teardown hang has no
counterpart here. `apps/clipse-notch` compiled for the first time (see below).

**2026-07-28, Debian 13 aarch64 (Raspberry Pi 5).** Both Linux backends ran for
the first time. X11 under `Xvfb :99`: three clips pushed with `xsel`, all
captured with correct previews and byte counts. **Wayland under a headless
`labwc`** (a wlroots compositor, so `wlr-data-control` is present): `wl-copy`
captured, and `clipse-ctl` reported `capture Automatic` — the real watch path,
not the manual-push fallback. Both secrets suppressed, confirmed by `strings`
over the database and WAL; `order 12345678901234 shipped` stored. `SIGTERM`
exits in about a second on both paths.

Two things worth knowing before repeating this. `cargo test --workspace` fails
on Linux without a display — `daemon_e2e` starts a real clipboard watcher, which
refuses to start when neither `DISPLAY` nor `WAYLAND_DISPLAY` is set. That is
what `xvfb-run -a` in the CI job is for; from a bare SSH session you have to
supply it yourself. And `xsel --clipboard --input` needs no `--keep`: it forks
into the background on its own to retain CLIPBOARD ownership, whereas `--keep`
only covers PRIMARY and SECONDARY.

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

It builds now — `swift build` was first run on a real Mac on 2026-07-28 and
failed, on `override func mouseEntered/mouseExited` in a controller that is an
`NSObject` and not an `NSResponder`. That is fixed. Do not trust `ci.yml` to
have caught things like this: the repository has no remote, so no workflow in it
has ever executed.

What remains needs eyes on a physical machine, because the window server is not
reachable over SSH. On a notched MacBook: the panel hovers open, shows the last
three clips, is
positioned from `NSScreen.safeAreaInsets` (verify on an external display too,
where there is no notch), accepts a drag-and-drop, and animates the source
device. It must not steal focus from the frontmost app.

## F4 — packaging

Install from the built artefact on a clean machine per platform. The installer
is signed (no SmartScreen warning on Windows, no Gatekeeper block on macOS),
the app launches, the daemon registers to start at login, and the updater
finds and applies a newer release.
