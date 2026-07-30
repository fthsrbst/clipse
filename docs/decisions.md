# Decisions

Short entries: what was decided, and why. Newest first.

## 2026-07: A frameless main window, with our own controls on every platform

**Decision.** `decorations: false` on the `main` window, on all three
platforms, and no replacement title bar. The masthead is the frame: the search
row carries the window controls on its baseline, and the spine's slack is the
drag region.

**Why.** On Windows the native title bar sat directly above the editorial
masthead — two top bars, one of them ours. Drawing our own title bar would be
the same problem in our own colours. Doing it per-platform would mean two
masthead variants to maintain for one window; the split is one config line away
if macOS turns out to need its traffic lights back.

**Consequence.** A frameless Win32 window has no resize border and no
double-click-to-maximize target. Both are replaced by hand — see
`lib/window-frame.ts` and `components/resize-handles.tsx`. The direction
mapping is unit-tested because it is the half that fails silently: a misspelled
direction does not throw, it makes one edge dead until someone grabs it. Aero
Snap under Tauri's drag emulation is unverified; the outcome goes in
`manual-verification.md`.

## 2026-07: `IPC_VERSION` 1 → 2, once, for two additions

**Decision.** One bump covering both `DaemonStatus::secrets_refused` and
`Request::GetPayload`. Neither gets an optional-field fallback or a degraded
path.

**Why.** The `Hello` handshake already refuses a connection whose peer reports a
different `IPC_VERSION` (`clipse-ipc/src/client.rs`), so after a bump both sides
are guaranteed to agree. Writing graceful degradation for an older daemon would
have been code for a case the handshake makes unreachable. The mismatch case is
a user running a standalone `clipsed` of a different version, and failing
loudly there is the existing, deliberate design.

**Consequence.** `clipse_core::PROTOCOL_VERSION`, which governs the *network*
format between daemons, is untouched — no `ClipFormat::label()` string and no
part of the content-hash layout changed.

## 2026-07: A 24MB preview cap, and `serde_bytes` on the payload response

**Decision.** `Request::GetPayload` returns at most `MAX_PAYLOAD_BYTES` (24MB),
as a `serde_bytes::ByteBuf`. The cap is checked before the store is asked.

**Why.** A cap at all, because a 400MB file copy is a perfectly good clip and a
terrible thing to pull into a webview. 24MB specifically, because it clears a
4K screenshot with 8MB of headroom under `MAX_FRAME_BYTES`. `serde_bytes`
because `rmp_serde` encodes a plain `Vec<u8>` as an array of integers at
roughly 1.5x — without it the cap would have been an artefact of the encoding
rather than a decision about the product.

**Consequence.** Checking the cap first is load-bearing: a payload declared
larger than the cap may have no blob on disk at all (an offer whose chunks never
arrived), and reading it would turn "too big to preview" into an error. The
Tauri layer re-encodes to base64, because the only consumer builds a `data:`
URL and Tauri would otherwise hand the webview a JSON array of numbers.

## 2026-07: The suppression count is never persisted

**Decision.** `DaemonStatus::secrets_refused` is an in-memory counter for the
life of the daemon process.

**Why.** A durable tally means writing a record about the thing Clipse promised
not to write down. "Since Clipse started" is an honest answer and costs
nothing. The count exists at all because the product's central promise
otherwise looks, from the outside, exactly like nothing happening.

## 2026-07: `clipse-store` encryption is a cargo feature, not always-on

**Decision.** At-rest encryption uses SQLCipher, enabled by the `encryption`
feature of `clipse-store`. The feature is off in the default dev profile and on
in CI and release builds.

**Why.** SQLCipher through `rusqlite` requires building OpenSSL, which on
Windows needs Perl and NASM. Neither is present on the primary dev machine, and
requiring a Strawberry Perl install to run `cargo test` would make the workspace
hostile to contributors. Whole-database encryption (rather than
application-level field encryption) is still the right call because FTS5 needs
plaintext to index — encrypting individual columns would cost us search.

**Consequence.** Shipped binaries are always built with `--features encryption`;
the release workflow fails if the feature is absent. A dev database is *not*
encrypted, so `.clipse-dev/` is gitignored and must never hold real history.

## 2026-07: One QUIC path, an ordered candidate address list

**Decision.** LAN and tailnet are not separate transports. Pairing records every
address a peer advertises (LAN + tailnet), and dialling walks the list in order
— LAN first, tailnet as fallback — over the same QUIC stack.

**Why.** Two transports would mean two session state machines, two sets of
failure modes and two places for the loop guard to be wrong. A tailnet address
is just another socket address.

## 2026-07: A clip is a set of representations, not one blob

**Decision.** `Clip` holds `Vec<Payload>`, one per format, and its
`ContentHash` covers all of them.

**Why.** A single copy from a word processor puts text, HTML and RTF on the
clipboard simultaneously. Modelling that as three clips would triple the
history; modelling it as text-only would silently downgrade every paste into a
rich editor.

## 2026-07: Separate daemon process (`clipsed`) from the UI

**Decision.** Sync, capture and storage live in a background daemon; the Tauri
app is a client over a unix socket / named pipe.

**Why.** The product promise is that a copy on the desktop is on the laptop
seconds later. That cannot depend on a window being open, and a webview process
is the wrong thing to keep resident.
