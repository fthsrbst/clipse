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

**Remaining:** one wire-up. The ceremony runs over the network on its own
ALPN, the daemon-side state machine that decides when a pairing may be
committed is written and tested (`clipsed/src/pairing.rs`), and the IPC
messages exist. What is missing is handing the daemon the device key, the
trust set and the transport together so it can drive them. Until then
`BeginPairing` and friends answer `Unsupported`, and devices are paired by
recording each other's details directly.

That last step was left rather than rushed: committing a pairing is the
security boundary of the whole product, and a half-verified path there is
worse than an honest refusal.

## F3 — macOS notch

`ClipseNotch` Swift sidecar: borderless `NSPanel` at `.statusBar` level,
positioned from `NSScreen.safeAreaInsets`, hover to open, last three clips,
source-device animation, drop target.

**Blocked on hardware.** None of this can be built or checked without a Mac,
and a notch panel is exactly the kind of thing that has to be seen to be
believed. The macOS clipboard backend is type-checked against the target and
built by CI, but has never been run.

## F4 — Ship it

Signed MSI, notarised DMG, AppImage + .deb, Tauri auto-updater, release
workflow.

**Blocked on credentials.** Signing needs an Authenticode certificate and an
Apple Developer ID that only the project owner can hold. The build and
packaging steps can be written and dry-run unsigned; the signing and
notarisation steps cannot be verified without them.

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
| `clipsed` | 54 | 11 sync two full daemon stacks; 6 drive the real binary |
