# Window frame, ASCII identity and the editorial grid

**Date:** 2026-07-30
**Status:** approved, not yet implemented

## Why

The onboarding sequence works. It has a voice — asymmetric composition, a
rotated spine, display type carrying the hierarchy, an eclipse computed in
characters. Nothing else in the product speaks that language. The history
window is a header, a toolbar, a list and a footer; settings is a stack of
`label | control` rows. Both are competent and anonymous.

Three concrete faults sit underneath that:

1. **The mark is a lie.** `components/ascii-mark.tsx` is SVG geometry named
   ASCII. A working character-grid eclipse already exists in
   `lib/eclipse-ascii.ts` as `ECLIPSE_MARK` and is dead code.
2. **The app opens more than once.** There is no single-instance guard. The
   second process cannot bind the IPC endpoint, so it becomes a client of the
   first one's daemon: two windows, one daemon, and the strong impression of
   having started a second session.
3. **Windows shows two top bars.** The `main` window keeps its native
   decorations, so the OS title bar sits directly above the editorial masthead.

## Goals

- The identity is drawn in characters everywhere a character grid can survive.
- One running app, one window, on every platform.
- No OS title bar above our masthead, and no separate title bar of our own
  either — the masthead *is* the frame.
- History and settings inherit the onboarding's composition.
- A clip's full content is reachable, including a screenshot.
- The privacy promise becomes visible without ever storing what it refused.

## Non-goals

- A light theme. Deliberately absent; the reasoning is in `styles/tokens.css`
  and is not being revisited here.
- Changing `ClipFormat::label()` strings or the content-hash layout. Nothing in
  this work touches either, so `clipse_core::PROTOCOL_VERSION` stays put.
- Touching the sync path, the crypto, or the store schema.
- Redesigning the hotkey popup. It is summoned mid-task and is already tuned
  for that; it inherits only the new ASCII tokens.

---

## A. Window frame

`main` gets `"decorations": false` in `tauri.conf.json`. It does **not** get a
custom title bar component. That distinction is the whole design: a frameless
window with a drawn title bar is the same two-bar problem with extra steps.

- **Drag region.** `data-tauri-drag-region` on the spine's empty middle span
  and on the padding around the search row. Both are areas that exist for
  composition anyway, so the frame costs no vertical space.
- **Window controls.** `–  ▢  ✕` as mono characters, no boxes, aligned to the
  same baseline as the search row, top right. Only `✕` tints on hover, using
  `--color-accent`; per the token rules only one thing on screen is red, and a
  hovered close button is the thing about to happen.
- **macOS.** Traffic lights are gone with the decorations; our controls take
  their place. This is the consistency that was chosen over native feel.

### What frameless costs on Windows, and the mitigation

A frameless Win32 window loses its resize border and its Aero Snap affordance.
Neither is optional to replace:

- Eight invisible 8px edge/corner handles calling
  `getCurrentWindow().startResizeDragging(direction)`.
- Double-click on the drag region toggles maximize.
- Snap behaviour under `startDragging` is **unverified** and must be driven by
  hand on Windows 11 before this area is called done. If Tauri's drag emulation
  does not produce snap, the fallback is a keyboard path (`Win`+arrows already
  works at the OS level) plus the maximize toggle, and the limitation gets
  written into `docs/manual-verification.md` rather than hidden.

### Permissions

`src-tauri/capabilities/default.json` adds `core:window:allow-minimize`,
`core:window:allow-toggle-maximize`, `core:window:allow-close`,
`core:window:allow-start-dragging`, `core:window:allow-start-resize-dragging`.

---

## B. Single instance

Add `tauri-plugin-single-instance` (v2), registered **first** in the builder
chain — the plugin requires this to intercept before other setup runs.

The second process hands its argv to the first and exits. The first:

- `unminimize()`, `show()`, `set_focus()` on `main`;
- **unless** the incoming argv contains `--minimised`. A login item that fires
  twice must not throw a window at someone who is still signing in. This mirrors
  the existing `--minimised` handling in `lib.rs:99`.

`clipsed` as a standalone process is untouched. Whoever binds the IPC endpoint
first is the daemon and the app is its client; that architecture stays. What is
being made single is the *app*, not the daemon.

---

## C. ASCII identity

**Deleted:** `components/ascii-mark.tsx`, `components/ascii-mark.module.css`,
`components/eclipse-mark.tsx`, `components/eclipse-mark.module.css`. After the
substitutions below, `EclipseMark` has no callers; leaving a second, SVG source
of truth for the mark is how the two drift apart.

**Added:**

- `lib/ascii-logotype.ts` — exports the revived `ECLIPSE_MARK` (moved here from
  `eclipse-ascii.ts`, which keeps the *computed* field) and a new
  `CLIPSE_WORDMARK`: `CLIPSE` as block characters. Both are arrays of
  equal-length strings. Equal length is load-bearing: a short row makes the
  logo lean.
- `components/ascii-logo.tsx` — `variant: "mark" | "wordmark" | "lockup"`,
  plus `scale`. Renders a `<pre>` with `role="img"` and `aria-label="Clipse"`,
  under `line-height: 1`, `letter-spacing: 0`,
  `font-variant-ligatures: none`. DM Mono ligatures would fuse `=*` pairs and
  bend the grid.

**Substituted:** `empty-state.tsx` and `daemon-offline-state.tsx` drop the SVG
disc for `eclipse-ascii.render()`. The loading variant reuses the phase drift
already implemented in `eclipse-canvas.tsx`; the offline variant is a single
static frame, because a machine that cannot reach its daemon should not look
busy.

**The 16px problem.** The deleted component's comment was right: a nine-row
eclipse is a smudge beside a wordmark. The answer is not a better ASCII mark,
it is not asking for one — the mark is never rendered below 44px. The spine has
the vertical room and renders it at ~56px. Tray and OS icons stay SVG-derived
`.ico`/`.icns`; an operating system will not take a character grid, and that is
a fact rather than a compromise.

---

## D. History window: spine and hanging edge

`grid-template-columns: 4.5rem 1fr`, with a third column when the peek panel is
open.

**Spine** — full height, hairline on its right, `--color-surface-sunken`. Top
to bottom: ASCII eclipse mark, rotated vertical `CLIPSE` wordmark, the clip
count as display type with tabular figures under a `CLIPS` label, the refused-
secrets count, and at the foot the settings control and a dot per paired
device. The gap between the count block and the foot is the drag region.

**Main column**

- The search field is not a box. A bottom hairline, `--type-md`, no border, no
  background. Filter tabs sit to its right as small uppercase mono with
  `--tracking-label`; the active one is marked by an accent underline, not a
  pill.
- Rows stay 56px. The grid becomes
  `[glyph 1.5rem] [content 1fr] [device 6rem] [time 4rem] [actions auto]`, and
  the kind indicator becomes a **mono character**, not an SVG icon — the row is
  a line of set text, and an icon in it is a foreign object.
- A pinned clip gets an accent tick in the left margin, *outside* the content
  column. That overhang is the hanging edge.

**Removed:** the footer. `Paused` and `Loading more…` move into the spine as
mono lines. An empty status bar is a row of chrome earning nothing.

**Settings stops being a screen.** The spine stays mounted, the settings
control becomes active, the `← Back` button is deleted, and Escape returns to
the history.

---

## E. Peek panel

**Interaction.** Space or `→` opens it on the selected clip; Escape or Space
closes it. Third grid column at `min(38%, 420px)`. Below 760px viewport width
it becomes an overlay over the list instead of a column, so the list never
gets squeezed into unreadability. `minWidth` in `tauri.conf.json` rises from
640 to 720 — deliberately *below* the 760px switch, so the overlay mode is a
real, reachable state and not dead code that only a resize bug could enter.

**Content.** Full text in mono and scrollable, or the image, or the file list.
Beneath it a colophon-style block in `--type-2xs` mono: source application,
device label, absolute timestamp, size and format per payload, and the first
12 hex characters of the BLAKE3 digest with the full value copyable.

**E.3 — `Request::GetPayload`.** `GetClip` returns a `Clip`, and
`PayloadBody::Blob` carries no bytes: anything over
`clipse_core::INLINE_MAX_BYTES` (64KB) is unreachable from the webview. This
is already documented at `lib/clip-content.ts:23`. Screenshots are almost
always over that line, so without a fix the panel shows a size where the
picture should be.

Add one request/response pair to `clipse-ipc`:

```rust
Request::GetPayload { id: ClipId, format: ClipFormat }
Response::Payload(Option<Vec<u8>>)
```

The daemon resolves the clip, finds the matching payload, and reads the CAS
blob by digest. Notes that belong in the implementation:

- This is an IPC wire change, not a *network* protocol change.
  `clipse_core::PROTOCOL_VERSION` governs peer sync and is not bumped;
  `clipse-ipc`'s own version is. A newer app against an older standalone daemon
  gets an unknown-request error, and the panel must degrade to the size-only
  placeholder rather than showing an error state.
- A cap on the returned size, so a 400MB file copy cannot be pulled into a
  webview as a `data:` URL. Above the cap the panel reports the size and
  offers nothing else.

---

## F. Refused-secrets counter

The pipeline already exists and is unused at both ends. `clipsed` receives
`CaptureEvent::Suppressed` (`crates/clipsed/src/capture.rs:22`) and emits
`Event::Suppressed { reason }`; the frontend's `onSuppressed`
(`lib/tauri-client.ts:132`) has no callers.

Added: `secrets_refused: u64` on `DaemonStatus` — a process-lifetime counter
held in memory. Not persisted, and carrying no content, ever. "Since Clipse
started" is an honest thing to say and needs no storage; a durable count would
mean writing about the thing we promised not to write about.

The frontend reads the field optionally so an older daemon's `DaemonStatus`
still deserializes and simply reports nothing. Live increments come from the
existing event.

Rendered in the spine as a count with no content. Zero is rendered muted rather
than hidden: "nothing has been refused" is also information.

**On the colour.** The token rules allow one red thing on screen, and the close
control already claims red on hover. So a standing count is set in normal ink,
never accent, however high it goes. Accent appears only as a brief flash at the
moment the count increments — a suppression that just happened is exactly the
"thing that just happened" the accent exists for, and it is gone before a
hovered close button could contend with it.

---

## G. Verification

The existing 266 Rust, 56 vitest and 9 Playwright tests stay green, and
`cargo clippy --workspace --all-targets -- -D warnings`, `cargo fmt --all` and
`tsc` stay clean.

Added:

- **vitest** — every row of `CLIPSE_WORDMARK` and `ECLIPSE_MARK` is the same
  length. This is the failure mode that makes a logo lean and the one a human
  eye forgives for weeks.
- **vitest** — the peek panel's payload selection: inline text renders, a
  blob-backed payload asks for bytes, an over-cap payload does neither.
- **Playwright** — peek opens on Space and closes on Escape; the spine renders
  the wordmark as *text* (assert the `<pre>` content, which is the only way to
  prove the logo is ASCII and not an image again); window controls exist.
- **Rust** — `secrets_refused` increments once per suppression and appears in
  `DaemonStatus`.

**Manual, on Windows 11, recorded in `docs/manual-verification.md`:** resize
from all eight edges, double-click maximize, drag-to-snap, minimize/close via
our controls, and a second launch focusing the first window instead of opening
another. None of these are testable from here, so none of them will be claimed
as working until they have been driven.

---

## Deferred

Not in this pass, and not lost:

- Text transforms on a clip — trim, case, JSON pretty-print, strip formatting,
  base64/URL decode. All pure functions; the most-requested clipboard-manager
  feature.
- A `?` keyboard cheat-sheet, and a "show the introduction again" control in
  settings. The onboarding flag is written to `localStorage` with no way back,
  so today those screens are seen once per machine and never again.
- Sequential paste (a copy stack).
- Named snippets.
- Filters by device, date and source application.
- A Windows jump list.

## Correction to the project map

`styles/tokens.css` is the truth: **Bricolage Grotesque** (variable, to 800)
and **DM Mono**. The hub project map still says Instrument Serif + IBM Plex
Mono. This design follows the code; the map gets fixed at the end of the
session.
