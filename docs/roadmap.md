# Roadmap

Each phase ends with something that runs, has passing tests, and has been
checked by hand.

## F0 — Skeleton and CI ✅

- Cargo workspace, eight crates, shared domain types in `clipse-core`
- HLC, content hashing and the clip model with unit tests
- GitHub Actions matrix across Windows, macOS and Linux

## F1 — One device

- `clipse-clipboard`: `Clipboard` trait, Windows / macOS / X11 / Wayland
  watchers, sensitive-content suppression
- `clipse-store`: SQLite + FTS5 history, content-addressed blobs, LRU quota
- `clipse-ipc` + `clipsed`: daemon that captures, stores and serves history
- `clipse-app`: tray, history window, global hotkey popup with fuzzy search and
  synthetic paste

## F2 — Two devices

- `clipse-crypto`: device keys, QR + six-digit pairing, Noise_IK sessions,
  rotation when a device is removed
- `clipse-net`: mDNS discovery, QUIC transport, tailnet candidate addresses
- `clipse-sync`: HLC merge, loop guard, offer/chunk transfer for large clips
- End-to-end test: two daemons, one clip, both histories

## F3 — macOS notch

- `ClipseNotch` Swift sidecar, borderless `NSPanel` at `.statusBar` level
  positioned from `NSScreen.safeAreaInsets`
- Hover to open, last three clips, source-device animation, drop target

## F4 — Ship it

- Brand assets, icon set, motion
- Signed MSI, notarised DMG, AppImage + .deb
- Tauri auto-updater and release workflow
