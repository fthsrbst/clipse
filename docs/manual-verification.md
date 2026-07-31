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

**2026-07-31, Windows 11 — the installer artwork, seen.** `0.2.1` built with
`--bundles nsis,msi` and both wizards driven far enough to look at every image
they carry. Nothing was installed; both were cancelled, and the MSI's own exit
page confirmed "your system has not been modified".

Verified: the NSIS Welcome sidebar (the eclipse, not NSIS's grey-blue default);
the NSIS header plate on the License page; the MSI Welcome dialog with its
heading and body text fully legible over the light zone; the MSI banner on the
License page with the page title clear of the wordmark; and the MSI exit dialog,
which reuses the same dialog bitmap.

One thing written down wrong and now corrected: the NSIS header image lands at
the **left** of the header strip, not the right. MUI2 only moves it right when
`MUI_HEADERIMAGE_RIGHT` is defined, and Tauri's template does not define it.
The plate works either way, but the claim was wrong.

Still unseen: the NSIS Finish page (it is bound to the same
`MUI_WELCOMEFINISHPAGE_BITMAP` as Welcome, so it is the same image — but that
is an argument, not an observation) and the uninstaller header, both of which
need a completed install. Everything on macOS.

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

**2026-07-31, macOS 26.5.2 (arm64) — the first *installed* copy, from a dmg.**
Everything before this ran from `cargo`/`pnpm tauri dev`; this was the shipped
artefact, dragged into `/Applications` from the disk image window, with the
download flag set on the dmg by hand so Gatekeeper would treat it as downloaded.

What it found, in order of how badly it mattered:

1. **The published app could not be opened at all.** Not a warning — "Clipse is
   damaged and can't be opened. You should move it to the Trash." The cause was
   in the release workflow, not in the app: with no Developer ID, Tauri signed
   nothing, so the bundle had no seal and `codesign --verify` rejected it.
   `v0.2.1` and `v0.2.2` both shipped this way. Fixed by signing ad-hoc; see
   `docs/packaging.md`.
2. **The dmg published for `v0.2.2` had no `.DS_Store`,** so none of the
   installer artwork was ever displayed — the background image was on the
   volume, unused, and the window opened as a default Finder window. CI-built
   dmgs had never been styled. Also fixed in the workflow.
3. Once built locally with the styling applied, items 7 and 8 hold: the window
   is 660×420, the eclipse is there, and both icons sit inside their clearings.
   Item 9 — the drag reads without an arrow. No chevrons needed.
4. **The window is 28 points too short**: Finder's bounds include the title bar,
   so a 420-tall window shows 392 points of a 420-tall drawing. `windowSize` is
   now 448. Verified by resizing the live window to 448 and watching the bottom
   of the field arrive.
5. **Finder draws the icon labels in near-black on the near-black artwork**, so
   "Clipse" and "Applications" are effectively invisible until an icon is
   selected. Not fixed — it is a design decision, and the room being black is
   the design. Recorded here so the next person does not rediscover it as a bug.

The app itself, installed: onboarding runs through its four cards, the main
window opens, the daemon inside it connects with no "isn't running" state, and
the spine reads `1 CLIP`. Copying an AWS key and a Luhn-valid card number left
the history at one item, and `strings` over `clipse.db`, `-wal` and `-shm` found
neither of them — the privacy promise, checked at the bytes, on an installed
copy. The refused counter stayed at `0` while doing it, which was a real bug:
the daemon counted the refusals but never pushed a status update, so the one
number that has to be believable was stale. Fixed in `crates/clipsed/src/capture.rs`.

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

## The frameless window

`decorations: false` removed the OS title bar, and with it everything the OS
was providing for free. None of this can be tested from a headless runner — a
Playwright page has no window manager, so it proves the controls *render* and
nothing about what they do.

Run `pnpm tauri dev` and drive all seven, then record the outcome here,
including anything that does not work:

1. No OS title bar above the masthead.
2. Dragging the spine's empty middle, or the space around the search row, moves
   the window.
3. Double-clicking a drag region toggles maximize.
4. All four edges and all four corners resize.
5. Dragging to the top of the screen snaps to maximize; dragging to a side
   snaps to half. **This is the genuinely uncertain one.** If Tauri's drag
   emulation does not produce Aero Snap, write that down rather than papering
   over it — `Win`+arrow still works at the OS level, and a known limitation
   beats a silent one.
6. Minimize, maximize and close all work, and close hides to the tray rather
   than quitting.
7. Launching a second copy focuses the first window instead of opening another
   — the half of the single-instance guard that has no test.

**2026-07-30, Windows 11 — item 7 only.** Uninstalled 0.1.0, installed the
0.2.0 NSIS build, launched it, then launched it again from the same path. The
process count stayed at 1, so the guard holds in a real installed build and not
just in the unit test for `should_reveal`. Items 1–6 are still unrun: they need
a hand on the mouse, and the run that would have shown them was refused screen
access.

Worth recording separately: the 0.1.0 uninstaller removed
`%LOCALAPPDATA%\Clipse` and left `%APPDATA%\clipse\Clipse\data` untouched, so
history, `config.toml` and `identity.json` all survived. Device identity
surviving an upgrade is what stops an upgrade from silently unpairing every
other machine.

## F4 — packaging

Install from the built artefact on a clean machine per platform. The installer
is signed (no SmartScreen warning on Windows, no Gatekeeper block on macOS),
the app launches, the daemon registers to start at login, and the updater
finds and applies a newer release.

## Installer artwork

`src/test/installer-assets.test.ts` proves the five images exist, are the exact
sizes NSIS and WiX demand, and are still referenced from `tauri.conf.json`. It
cannot prove any of them is *displayed*, which is a different claim and the one
that matters. Each item below has to be looked at.

**Windows.**

```bash
cd apps/clipse-app && pnpm tauri build --bundles nsis,msi
```

1. Run the NSIS `-setup.exe`. The Welcome page shows the eclipse sidebar down
   its left edge, not NSIS's grey-blue default.
2. Advance past Welcome. The header strip carries the dark `CLIPSE` plate at
   its **left**, with the page title set beside it — MUI2 puts the header image
   on the left unless `MUI_HEADERIMAGE_RIGHT` is defined, and Tauri's template
   does not define it.
3. Finish the install. The Finish page shows the same sidebar as Welcome.
4. Run the uninstaller from Add/Remove Programs. Its header is the same plate —
   an uninstaller that looks like a different program is its own small alarm.
5. Run the `.msi`. The Welcome dialog's left band is the eclipse and **the
   heading and body text are readable** — that is the whole reason the right of
   that bitmap is light.
6. Advance a page in the MSI. The banner's page title does not collide with the
   wordmark at the right edge.

**macOS — done on 2026-07-31, on a real Mac. Read the log below first: items
7–9 were looked at, and two of them found something.**

7. Open the `.dmg`. The window is 660×420 and shows the eclipse.
8. The Clipse icon and the Applications folder each sit *inside* a clearing in
   the corona rather than on top of the characters. If they do not, the
   positions in `tauri.conf.json` and the `DMG` constants in
   `scripts/render-installer-art.mts` have drifted apart; they are two halves of
   one drawing.
9. Judge whether the drag reads without an arrow. If it does not, the fallback
   is chevrons built from ramp characters along the same line — decided at the
   machine, not before it.
10. The background will be soft on a Retina display. Tauri accepts only
    `png`/`jpg`/`gif` here, not a multi-representation `.tiff`, so there is no
    `@2x` layer to supply. Confirm it is *soft*, not *wrong*.
