# Roadmap

Each phase ends with something that runs, has passing tests, and has been
driven by hand. `docs/manual-verification.md` is that checklist.

## F0 — Skeleton and CI ✅

- Cargo workspace, nine crates, shared domain types in `clipse-core`
- HLC, content hashing and the clip model, unit tested
- GitHub Actions: three-platform matrix, a SQLCipher job, a cross-check job —
  **written, never executed.** There is no remote to dispatch them, so every
  workflow in `.github/` is unproven, and so is anything it was expected to
  prove. The local commands in `CLAUDE.md` are the only evidence there is.

## F1 — One device

**Daemon: done and verified end to end.**

- `clipse-clipboard` — Windows (`AddClipboardFormatListener`), macOS
  (`NSPasteboard.changeCount`), X11 (XFIXES), Wayland (`wlr-data-control`),
  GNOME Wayland manual-push. Sensitive-content suppression: platform concealed
  markers, app blocklist, secret detectors. All four watch paths have now been
  executed on real hardware, not merely compiled — Windows, macOS 26 on arm64,
  and X11 plus wlroots Wayland on aarch64 Debian. Suppression was checked on
  each by searching the database bytes, not by asking the daemon.
- `clipse-store` — SQLite + FTS5 history, content-addressed blobs, LRU quota
  that never touches text or pinned clips.
- `clipse-ipc` + `clipsed` — capture → store → serve, over a unix socket or a
  named pipe. Six end-to-end tests drive the real binary.
- `clipse-app` (Rust half) — connection with backoff, tray, global hotkey,
  popup positioning, a mock daemon for development.

**Remaining:** the pairing screen and the hotkey popup have never been driven by
hand — the history window has (2026-07-28, Windows), and it reconnects on its own
when the daemon comes back. A password manager still has to be tested against
the real thing by a person.

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

**F2 is complete**, including the pairing screen in Settings. It shows the
offer, takes a pasted one from the other device, puts the six digits on screen
and asks whether they match — and the button that commits the pairing is
disabled anywhere the code is not visible, which the reducer behind it is
tested for exhaustively.

The offer appears as a QR code *and* as a copyable string. The string is not a
fallback nobody uses: pairing a desktop to a desktop means copying text,
because neither has a camera pointed at the other.

The QR is rendered to SVG in the Tauri command by a compiled-in encoder. The
rule this app follows is that nothing reaches out at runtime — no CDN, no
fonts, no telemetry — and a Rust dependency does not violate it.

## F3 — macOS notch

`apps/clipse-notch` is written: a borderless `NSPanel` at `.statusBar` level
that never takes focus, positioned from `NSScreen.safeAreaInsets` so an
external monitor gets sensible placement rather than a panel floating below
the menu bar, hover to expand, three clips, an arrival animation for anything
that came from another device, and a text drop target.

It is a sidecar rather than part of the Tauri app because a borderless AppKit
panel is not something a webview can be, and because a crash in a decoration
should not take the clipboard with it. It speaks newline-delimited JSON on
stdin/stdout rather than the daemon's protocol, so the wire format exists in
one place instead of two.

`clipse-app` launches it and streams the three most recent clips in as
newline-delimited JSON, reading pastes back out — so it is wired in rather
than an orphan file. That bridge is `src-tauri/src/notch.rs`, compiled only on
macOS.

It was written on Windows with no Swift toolchain, and the Tauri app cannot be
cross-checked for macOS from this machine because its Objective-C dependencies
need a C compiler for the target.

`ci.yml` does define a macOS job that runs `swift build`, and the macOS leg of
the test matrix would build the bridge — but **none of it has ever run**: this
repository has no git remote, so no workflow in it has ever been dispatched.
Nothing here was ever compiled by CI, and no claim in these docs should lean on
CI as evidence.

The Swift package was first compiled on a real Mac on 2026-07-28 and did not
build; see `docs/manual-verification.md` §F3. Whether the panel sits correctly
under a notch remains a question only someone looking at a MacBook can answer.

Bundling `ClipseNotch` into the .app is part of the packaging that has also
never been run; see `docs/packaging.md`.

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
