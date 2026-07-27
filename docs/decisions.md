# Decisions

Short entries: what was decided, and why. Newest first.

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
