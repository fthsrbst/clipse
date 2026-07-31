# Clipse

Clipboard sync for Windows, macOS and Linux. No server, no account, no cloud.

Copy on one machine, paste on another. On a LAN the devices talk directly over
QUIC after mDNS discovery; away from home they reach each other over a Tailscale
tailnet. Everything is end-to-end encrypted between devices that you paired
yourself with a QR code and a six-digit code.

## Status

Early development. See [docs/roadmap.md](docs/roadmap.md) for what is done and
what is next.

## Layout

| Path | What lives there |
| --- | --- |
| `crates/clipse-core` | Shared domain types: clips, content hashes, HLC, device ids |
| `crates/clipse-clipboard` | `Clipboard` trait + per-platform watchers |
| `crates/clipse-store` | Encrypted SQLite history, FTS5 search, content-addressed blobs |
| `crates/clipse-crypto` | Device keys, pairing, Noise sessions, key rotation |
| `crates/clipse-net` | Transport abstraction, QUIC, mDNS discovery, Tailscale resolution |
| `crates/clipse-sync` | Merge rules, loop guard, chunked transfer |
| `crates/clipse-ipc` | Daemon ⇄ UI protocol over unix socket / named pipe |
| `crates/clipsed` | The background daemon |
| `apps/clipse-app` | Tauri v2 desktop UI |
| `assets/` | Vector brand assets |

The daemon is separate from the UI on purpose: closing the window must not stop
syncing.

## Installing

Downloads are on the [releases page](https://github.com/fthsrbst/clipse/releases).

**macOS.** The builds are signed ad-hoc, not with an Apple Developer ID, so a
copy you downloaded is not notarised and macOS will not open it on the first
double-click. Open it once with **right-click → Open**, or clear the download
flag yourself:

```bash
xattr -dr com.apple.quarantine /Applications/Clipse.app
```

After that it launches normally, and updates do not ask again. The dmg is
Apple-silicon only for now.

## Privacy

Non-negotiable, and enforced in code rather than in settings:

- Password-manager copies are never stored and never synced. Clipse honours
  `org.nspasteboard.ConcealedType` (macOS),
  `ExcludeClipboardContentFromMonitorProcessing` (Windows) and
  `x-kde-passwordManagerHint` (Linux), plus an app blocklist and a detector for
  API keys, JWTs and card numbers.
- No accounts, no telemetry, no relay server. Content only ever travels between
  your own paired devices.

## Building

Requires a stable Rust toolchain (1.90+), Node 22 and pnpm.

```bash
cargo test --workspace
```

Platform prerequisites for the desktop app are the standard
[Tauri v2](https://v2.tauri.app/start/prerequisites/) ones.

## Licence

MIT
