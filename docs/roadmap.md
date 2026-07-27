# Roadmap

Each phase ends with something that runs, has passing tests, and has been
driven by hand. `docs/manual-verification.md` is that checklist.

## F0 — Skeleton and CI ✅

- Cargo workspace, nine crates, shared domain types in `clipse-core`
- HLC, content hashing and the clip model, unit tested
- GitHub Actions: three-platform matrix, a SQLCipher job, a cross-check job

## F1 — One device

**Daemon: done and verified end to end.**

- `clipse-clipboard` — Windows (`AddClipboardFormatListener`), macOS
  (`NSPasteboard.changeCount`), X11 (XFIXES), Wayland (`wlr-data-control`),
  GNOME Wayland manual-push. Sensitive-content suppression: platform concealed
  markers, app blocklist, secret detectors.
- `clipse-store` — SQLite + FTS5 history, content-addressed blobs, LRU quota
  that never touches text or pinned clips.
- `clipse-ipc` + `clipsed` — capture → store → serve, over a unix socket or a
  named pipe. Six end-to-end tests drive the real binary.
- `clipse-app` (Rust half) — connection with backoff, tray, global hotkey,
  popup positioning, a mock daemon for development.

**Remaining:** the React frontend, and running the F1 manual checklist on
Windows.

## F2 — Two devices

- `clipse-crypto` ✅ — device identity, QR + six-digit SAS pairing, Noise_IK
  sessions, trust epochs that invalidate a removed device.
- `clipse-sync` ✅ — merge rules, the three-layer loop guard, chunked transfer
  with resume and digest verification.
- `clipse-net` ✅ — candidate addresses, tailnet resolution, mDNS record codec,
  the QUIC transport with Noise_IK inside it, and the pairing ceremony on its
  own ALPN.
- `clipsed` ✅ — device key and paired set on disk, the sync driver, and the
  peer manager. The daemon listens on QUIC and syncs; text, rich text and
  blob payloads all cross.

mDNS announce and browse are wired in: the daemon advertises itself and folds
what it finds into the candidate list of devices it already trusts. Discovery
never adds a peer — an advertisement is an address for someone you already
paired with, not an invitation.

Pairing works end to end over IPC: one device shows a QR payload, the other
answers it over the network, both compute six digits, and neither trusts
anything until both are told the digits matched. Three tests drive two real
daemon processes through it — including refusing the code, which must pair
nothing.

**F2 is complete.** What is left in the app is a pairing *screen*: the IPC
messages and events are in place (`BeginPairing`, `PairWithUri`,
`ConfirmPairing`, `CancelPairing`, and the `pairing-code` / `pairing-ended`
events reach the webview), but no UI renders them yet.

## F3 — macOS notch

`ClipseNotch` Swift sidecar: borderless `NSPanel` at `.statusBar` level,
positioned from `NSScreen.safeAreaInsets`, hover to open, last three clips,
source-device animation, drop target.

**Blocked on hardware.** None of this can be built or checked without a Mac,
and a notch panel is exactly the kind of thing that has to be seen to be
believed. The macOS clipboard backend is type-checked against the target and
built by CI, but has never been run.

## F4 — Ship it

The release workflow, the bundle configuration and the updater config are
written — see `docs/packaging.md`. A `v*` tag builds MSI, NSIS, DMG, .app,
AppImage and .deb across the three-platform matrix, and ships the daemon
alongside the app because closing the window must not stop syncing.

**Signing is blocked on credentials** the project owner holds: an Authenticode
certificate and an Apple Developer ID. The workflow reads them from secrets and
produces unsigned artefacts with a warning when they are absent, rather than
failing at the last step of a long build.

**The auto-updater is deliberately off.** It verifies a signature over every
release it downloads; enabling it without a real key would ship an update
channel that trusts anything.

**Nothing here has been built.** No installer has been produced, installed or
launched. Treat this phase as a plan until someone tags a release.

## Where the tests are

| Crate | Tests | Notes |
| --- | --- | --- |
| `clipse-core` | 27 | Hashing, HLC ordering, clip identity |
| `clipse-clipboard` | 48 | 3 against the real Windows clipboard |
| `clipse-store` | 24 | Quota, FTS, migration, concurrency |
| `clipse-ipc` | 11 | Framing, plus the real platform transport |
| `clipse-sync` | 34 | Merge convergence, loop guard, chunk assembly |
| `clipse-net` | 52 | Dial order, tailnet parsing, framing, real QUIC loopback |
| `clipse-crypto` | 21 | MITM simulation, SAS bias, tamper detection |
| `clipsed` | 57 | 11 sync two full daemon stacks; 9 drive the real binary |
