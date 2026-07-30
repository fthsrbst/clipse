# Window Frame and Editorial Grid Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Carry the onboarding's editorial voice into the history window and settings, replace the SVG mark with a real character-grid identity, make the app single-instance, and remove the native Windows title bar in favour of a frameless window whose masthead *is* the frame.

**Architecture:** Bottom-up. The three Rust changes (single instance, a suppression counter, a payload-fetch request) land first and independently, because each is verifiable without touching a pixel. Then the ASCII identity replaces both SVG marks. Then the frameless frame arrives while the old header still exists to host its controls. Only then is the header replaced by the spine layout, which the peek panel and the settings grid build on.

**Tech Stack:** Rust 2024 (workspace crates + Tauri v2), React 19 + TypeScript strict, CSS Modules, vitest, Playwright, GSAP.

Spec: `docs/superpowers/specs/2026-07-30-window-frame-and-editorial-grid-design.md`

## Global Constraints

- `clipse-core` is the contract: no I/O, no async, no platform code. Nothing in this plan modifies it.
- `ClipFormat::label()` strings and the content-hash layout are untouched, so `clipse_core::PROTOCOL_VERSION` is **not** bumped. `clipse_ipc::IPC_VERSION` **is** bumped, exactly once, in Task 2.
- Privacy rules are code: the suppression counter stores a count and never content, on disk or in memory.
- The UI never touches the store or the network — only `clipse-ipc` through Tauri commands.
- Errors: `thiserror` enums per crate, `anyhow` only in binaries.
- Comments explain constraints the code cannot show, not what the next line does.
- Frontend: TypeScript strict, kebab-case filenames, PascalCase types.
- Every commit: `git commit --no-gpg-sign`.
- Design tokens come from `apps/clipse-app/src/styles/tokens.css`. No one-off durations, colours or radii. Type face names: **Bricolage Grotesque** (display, variable to 800) and **DM Mono** (mono).
- Only one red thing on screen at a time. A standing count is normal ink; accent is reserved for the moment something happens and for the hovered close control.
- `MAX_PAYLOAD_BYTES = 24 * 1024 * 1024` (Task 3). `clipse_ipc::MAX_FRAME_BYTES` is 32MB and must not be raised.

### Two corrections to the spec, applied throughout this plan

Both were found while reading the code to write exact task steps, and both make the work smaller:

1. **The payload cap is 24MB, not the 16MB quoted in conversation, and the response uses `serde_bytes`.** `clipse-ipc` frames are MessagePack via `rmp_serde::to_vec_named` (`crates/clipse-ipc/src/codec.rs:41`), where a plain `Vec<u8>` encodes as an *array of integers* — roughly 1.5 bytes per byte, so a 16MB payload would land near 24MB of frame against a 32MB `MAX_FRAME_BYTES` ceiling, and the cap would really be an artefact of the encoding. `serde_bytes::ByteBuf` encodes as a MessagePack `bin` blob at 1:1, which lets the cap be a product decision (how large a clip is worth previewing) with 8MB of frame headroom left over.

2. **No graceful degradation against an older daemon, because the handshake already forbids one.** `crates/clipse-ipc/src/client.rs:48` refuses any connection whose `Hello` reports a different `IPC_VERSION`. Bumping `IPC_VERSION` to `2` once — covering both the new `DaemonStatus` field and the new request — means both sides are guaranteed to agree, so `secrets_refused` needs no optional read and `GetPayload` needs no fallback path. The mismatch case is a user running a standalone `clipsed` of a different version, and failing loudly there is the existing, deliberate design.

---

### Task 1: Single-instance guard

**Files:**
- Modify: `apps/clipse-app/src-tauri/Cargo.toml`
- Modify: `apps/clipse-app/src-tauri/src/lib.rs`
- Create: `apps/clipse-app/src-tauri/src/instance.rs`

**Interfaces:**
- Consumes: nothing.
- Produces: `instance::should_reveal(argv: &[String]) -> bool`, and a registered `tauri_plugin_single_instance` that reveals the `main` window on a second launch.

- [ ] **Step 1: Write the failing test**

Create `apps/clipse-app/src-tauri/src/instance.rs`:

```rust
//! What a second launch means.
//!
//! The window-revealing half needs a running Tauri app and is verified by
//! hand (see `docs/manual-verification.md`); the decision of *whether* to
//! reveal is pure, and is the part that can be wrong silently.

/// Whether a second launch should bring the existing window forward.
///
/// A login item that fires twice must not throw a window at someone who is
/// still signing in, so `--minimised` in the *incoming* argv means "stay in
/// the tray" — the same flag `lib.rs` honours on a cold start.
pub fn should_reveal(argv: &[String]) -> bool {
    !argv.iter().any(|arg| arg == "--minimised")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(args: &[&str]) -> Vec<String> {
        args.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn a_plain_second_launch_reveals_the_window() {
        assert!(should_reveal(&argv(&["clipse.exe"])));
    }

    #[test]
    fn a_login_item_relaunch_stays_in_the_tray() {
        assert!(!should_reveal(&argv(&["clipse.exe", "--minimised"])));
    }

    #[test]
    fn the_flag_is_recognised_in_any_position() {
        assert!(!should_reveal(&argv(&["clipse.exe", "--minimised", "extra"])));
    }
}
```

Add `mod instance;` to the module list at the top of `apps/clipse-app/src-tauri/src/lib.rs` (after `mod hotkey;`).

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p clipse-app instance::`
Expected: FAIL — `cannot find module` / the file is not yet wired, or a compile error naming `instance`.

- [ ] **Step 3: Confirm it passes once the module is wired**

Run: `cargo test -p clipse-app instance::`
Expected: PASS, 3 tests.

- [ ] **Step 4: Add the plugin dependency**

In `apps/clipse-app/src-tauri/Cargo.toml`, after the `tauri-plugin-autostart` line:

```toml
# Registered first in the builder chain: the guard has to intercept before any
# other setup runs, or the second process does work before handing off.
tauri-plugin-single-instance = "2"
```

- [ ] **Step 5: Register the plugin first in the builder chain**

In `apps/clipse-app/src-tauri/src/lib.rs`, replace the line `tauri::Builder::default()` and the `.plugin(tauri_plugin_opener::init())` that follows it with:

```rust
    tauri::Builder::default()
        // First in the chain, deliberately: this plugin decides whether this
        // process is the app at all, and everything below assumes it is.
        //
        // Without it a second launch could not bind the IPC endpoint, so it
        // became a *client* of the first instance's daemon — two windows, one
        // daemon, and every appearance of a second session.
        .plugin(tauri_plugin_single_instance::init(|app, argv, _cwd| {
            if !instance::should_reveal(&argv) {
                return;
            }
            if let Some(main) = app.get_webview_window("main") {
                let _ = main.unminimize();
                let _ = main.show();
                let _ = main.set_focus();
            }
        }))
        .plugin(tauri_plugin_opener::init())
```

- [ ] **Step 6: Verify the workspace still builds clean**

Run: `cargo clippy -p clipse-app --all-targets -- -D warnings`
Expected: no warnings, no errors.

- [ ] **Step 7: Commit**

```bash
git add apps/clipse-app/src-tauri/Cargo.toml apps/clipse-app/src-tauri/src/instance.rs apps/clipse-app/src-tauri/src/lib.rs Cargo.lock
git commit --no-gpg-sign -m "Make the app single-instance

A second launch could not bind the IPC endpoint, so it became a client of
the first instance's daemon: two windows over one daemon, which reads as a
second session. The guard reveals the existing window instead -- unless the
relaunch carries --minimised, because a login item firing twice must not
put a window in front of someone who is still signing in.

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>"
```

---

### Task 2: Refused-secrets counter

**Files:**
- Modify: `crates/clipse-ipc/src/lib.rs:28` (bump `IPC_VERSION`)
- Modify: `crates/clipse-ipc/src/protocol.rs:226-237` (`DaemonStatus`)
- Modify: `crates/clipsed/src/daemon.rs` (counter field, `status()`, a bump method)
- Modify: `crates/clipsed/src/capture.rs:22-28`
- Modify: `apps/clipse-app/src/types/ipc.ts:81-92`
- Modify: `apps/clipse-app/e2e/fixtures/clips.ts` (status fixture gains the field)

**Interfaces:**
- Consumes: nothing.
- Produces: `DaemonStatus.secrets_refused: u64` / `secrets_refused: number` on the TS side; `Daemon::note_suppression(&self)`. `IPC_VERSION == 2`.

- [ ] **Step 1: Write the failing test**

There is no `Daemon` test harness in `crates/clipsed/src/daemon.rs` — its `mod tests` exercises free functions like `materialize` against a bare `Store` (`store_in`, `text_clip`), and never constructs a `Daemon`. Do not invent one for this: the counter's whole point is that a real suppression increments it, and that path already has a test.

Extend the existing `a_detected_secret_never_reaches_the_history` test in `crates/clipsed/tests/daemon_e2e.rs:310`. It already copies an AWS-shaped key from another process and awaits the `Event::Suppressed` push; add a status assertion after that await:

```rust
        // The count is the *only* record of a refusal, and it is what the
        // window shows. Asserted here rather than in a unit test because the
        // thing worth proving is that a real suppression reaches it.
        match client.call(Request::Status).await.unwrap() {
            Response::Status(status) => {
                assert_eq!(status.secrets_refused, 1);
                // And nothing about what was refused reached the store, which
                // is the promise the whole product is built on.
                assert_eq!(status.clip_count, 0);
            }
            other => panic!("expected status, got {other:?}"),
        }
```

Note this test lives in the Windows-only `clipboard` module (it drives the real OS clipboard through PowerShell so the copy genuinely comes from another process). On other platforms it does not run, which is a pre-existing property of the suite and not something to work around here.

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p clipsed --test daemon_e2e a_detected_secret`
Expected: FAIL to compile — `no field secrets_refused on DaemonStatus`.

- [ ] **Step 3: Add the field to the wire type and bump the IPC version**

In `crates/clipse-ipc/src/lib.rs:28`:

```rust
/// Bumped to 2 for `DaemonStatus::secrets_refused` and `Request::GetPayload`.
/// The `Hello` handshake refuses a mismatch outright (see `client.rs`), which
/// is why neither addition needs an optional-field fallback.
pub const IPC_VERSION: u16 = 2;
```

In `crates/clipse-ipc/src/protocol.rs`, add as the last field of `DaemonStatus`:

```rust
    /// Captures dropped for looking like a secret, since this daemon started.
    ///
    /// A count, and only a count. It is deliberately not persisted: a durable
    /// tally would mean writing a record about the thing Clipse promised not
    /// to write down, and "since Clipse started" is an honest answer that
    /// costs nothing.
    pub secrets_refused: u64,
```

- [ ] **Step 4: Add the counter to the daemon**

In `crates/clipsed/src/daemon.rs`, add to the imports:

```rust
use std::sync::atomic::{AtomicU64, Ordering};
```

Add to the `Daemon` struct definition:

```rust
    /// Process-lifetime only; see `DaemonStatus::secrets_refused`.
    secrets_refused: AtomicU64,
```

Initialise it as `secrets_refused: AtomicU64::new(0),` in every `Daemon` constructor in the file.

Add an inherent method next to `fn status`:

```rust
    /// Records that a capture was dropped. The reason is logged and emitted;
    /// only the count is kept.
    pub fn note_suppression(&self) {
        self.secrets_refused.fetch_add(1, Ordering::Relaxed);
    }
```

In `fn status`, add to the returned `DaemonStatus` literal:

```rust
            secrets_refused: self.secrets_refused.load(Ordering::Relaxed),
```

- [ ] **Step 5: Count real suppressions in the capture loop**

In `crates/clipsed/src/capture.rs`, inside the `CaptureEvent::Suppressed(reason)` arm, add the bump immediately before the existing `daemon.emit(...)` call:

```rust
                daemon.note_suppression();
                daemon.emit(Event::Suppressed { reason: label });
```

This comes before the verification step deliberately: the test asserts `secrets_refused == 1` after a real suppression, so a counter nothing increments would leave it red.

- [ ] **Step 6: Run the test to verify it passes**

Run: `cargo test -p clipsed --test daemon_e2e a_detected_secret`
Expected: PASS on Windows. On macOS or Linux the test is compiled out; `cargo test -p clipsed` must still be green there.

- [ ] **Step 7: Mirror the field on the TypeScript side**

In `apps/clipse-app/src/types/ipc.ts`, add as the last field of `DaemonStatus`:

```ts
  /** Captures dropped for looking like a secret, since the daemon started.
   * A count only — the daemon never records what was refused. */
  secrets_refused: number;
```

In `apps/clipse-app/e2e/fixtures/clips.ts`, add `secrets_refused: 0,` to the exported `DaemonStatus` fixture so the suite still type-checks.

- [ ] **Step 8: Verify the whole workspace**

Run: `cargo test --workspace`
Expected: PASS, including the new test.

Run: `cargo clippy --workspace --all-targets -- -D warnings && cargo fmt --all -- --check`
Expected: clean.

Run: `cd apps/clipse-app && pnpm tsc --noEmit`
Expected: clean.

- [ ] **Step 9: Commit**

```bash
git add crates/clipse-ipc/src/lib.rs crates/clipse-ipc/src/protocol.rs crates/clipsed/src/daemon.rs crates/clipsed/src/capture.rs apps/clipse-app/src/types/ipc.ts apps/clipse-app/e2e/fixtures/clips.ts
git commit --no-gpg-sign -m "Count the captures refused for looking like secrets

The pipeline already existed at both ends and was used at neither: clipsed
receives CaptureEvent::Suppressed and emits Event::Suppressed, and the
frontend's onSuppressed had no callers. All that was missing was a number
the window could show.

Held in memory for the life of the process. A durable tally would mean
writing a record about the thing Clipse promised not to write down.

IPC_VERSION goes to 2; the Hello handshake refuses a mismatch, so this
needs no optional-field fallback.

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>"
```

---

### Task 3: `GetPayload` — reach the bytes a screenshot lives in

**Files:**
- Modify: `crates/clipse-ipc/Cargo.toml`
- Modify: `crates/clipse-ipc/src/protocol.rs` (`Request`, `Response`)
- Modify: `crates/clipse-ipc/src/lib.rs` (export `MAX_PAYLOAD_BYTES`)
- Modify: `crates/clipsed/src/daemon.rs` (handler arm)
- Modify: `apps/clipse-app/src-tauri/src/commands.rs`
- Modify: `apps/clipse-app/src-tauri/src/lib.rs` (`invoke_handler!`)
- Modify: `apps/clipse-app/src/lib/tauri-client.ts`
- Test: `crates/clipsed/src/daemon.rs` (`mod tests`)

**Interfaces:**
- Consumes: `IPC_VERSION == 2` from Task 2.
- Produces:
  - `Request::GetPayload { id: ClipId, format: ClipFormat }`
  - `Response::PayloadBytes(Option<serde_bytes::ByteBuf>)`
  - `clipse_ipc::MAX_PAYLOAD_BYTES: u64`
  - Tauri command `get_payload(id: ClipId, format: ClipFormat) -> Result<Option<String>, CommandError>` returning **standard base64**
  - `api.getPayload(id: string, format: ClipFormat): Promise<string | null>`

Why base64 rather than bytes across the Tauri boundary: the only consumer builds a `data:` URL, and Tauri serialises `Vec<u8>` to the webview as a JSON array of numbers — roughly four characters per byte. Base64 is both smaller and already the shape the `<img>` needs.

- [ ] **Step 1: Write the failing test**

The logic worth testing is payload selection, not the enum plumbing around it, so it goes in a free function beside `materialize` in `crates/clipsed/src/daemon.rs` — which is exactly how that file is already organised: its `mod tests` drives free functions against a bare `Store` via `store_in(dir)` and `text_clip(store, text)`, and never builds a `Daemon`.

Append to `mod tests` in `crates/clipsed/src/daemon.rs`:

```rust
    #[test]
    fn read_payload_returns_inline_bytes_without_touching_the_cas() {
        let dir = tempfile::tempdir().unwrap();
        let store = store_in(dir.path());
        let clip = text_clip(&store, "hello there");

        let got = read_payload(&store, &clip, ClipFormat::Text).unwrap();
        assert_eq!(got.as_deref(), Some(b"hello there".as_slice()));
    }

    #[test]
    fn read_payload_pulls_a_blob_backed_image_off_disk() {
        let dir = tempfile::tempdir().unwrap();
        let store = store_in(dir.path());

        // Over INLINE_MAX_BYTES, so the body is `Blob` and carries no bytes at
        // all -- which is the entire reason this function exists.
        let big = vec![9u8; (INLINE_MAX_BYTES + 1_000) as usize];
        let payload = Payload::new(ClipFormat::Png, big.clone());
        assert!(payload.is_blob(), "test premise");
        let digest = payload.digest;

        let clock = HlcClock::new(DeviceId::generate());
        let clip = Clip::new(
            vec![payload],
            ClipSource::new(DeviceId::generate(), "test"),
            clock.now(),
        );
        store.put_blob(&digest, &big).unwrap();
        store.insert(&clip).unwrap();

        let got = read_payload(&store, &clip, ClipFormat::Png).unwrap();
        assert_eq!(got.as_deref(), Some(big.as_slice()));
    }

    #[test]
    fn read_payload_declines_an_over_cap_payload() {
        let dir = tempfile::tempdir().unwrap();
        let store = store_in(dir.path());

        // Fabricated rather than allocated: a real 24MB buffer would make this
        // test cost more than everything else in the file combined.
        let mut payload = Payload::new(ClipFormat::Png, vec![1u8; 8]);
        payload.size = clipse_ipc::MAX_PAYLOAD_BYTES + 1;

        let clock = HlcClock::new(DeviceId::generate());
        let clip = Clip::new(
            vec![payload],
            ClipSource::new(DeviceId::generate(), "test"),
            clock.now(),
        );
        store.insert(&clip).unwrap();

        // None, not an error: "too big to preview" is an ordinary answer, and
        // the panel shows the size instead.
        assert_eq!(read_payload(&store, &clip, ClipFormat::Png).unwrap(), None);
    }

    #[test]
    fn read_payload_is_none_for_a_format_the_clip_does_not_have() {
        let dir = tempfile::tempdir().unwrap();
        let store = store_in(dir.path());
        let clip = text_clip(&store, "hello");

        assert_eq!(read_payload(&store, &clip, ClipFormat::Png).unwrap(), None);
    }
```

If `Payload::size` is not publicly assignable, drop the third test's mutation and instead assert the cap through the `size > MAX_PAYLOAD_BYTES` branch with a payload built at whatever the smallest allocation is that exceeds it — and say so in a comment, because a reader will otherwise wonder why the test allocates 24MB.

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p clipsed read_payload`
Expected: FAIL — `cannot find function read_payload`, `MAX_PAYLOAD_BYTES` not found.

- [ ] **Step 3: Add the dependency and the cap**

In `crates/clipse-ipc/Cargo.toml`, under `[dependencies]`:

```toml
# So a payload response encodes as a MessagePack `bin` blob at 1:1 rather than
# as an array of integers at ~1.5x. Without it the size cap below would be an
# artefact of the encoding instead of a decision about the product.
serde_bytes = "0.11"
```

In `crates/clipse-ipc/src/lib.rs`, next to the `MAX_FRAME_BYTES` re-export:

```rust
/// The largest payload the daemon will hand to a UI for preview.
///
/// A ceiling on *previewing*, not on storing or syncing: a 400MB file copy is
/// still a perfectly good clip, it just is not something to pull into a
/// webview. Sized to clear a 4K screenshot with room to spare while leaving
/// 8MB of headroom under `MAX_FRAME_BYTES`.
pub const MAX_PAYLOAD_BYTES: u64 = 24 * 1024 * 1024;
```

- [ ] **Step 4: Add the request and response variants**

In `crates/clipse-ipc/src/protocol.rs`, add to `Request` immediately after the `GetClip` variant:

```rust
    /// The bytes of one of a clip's payloads.
    ///
    /// `GetClip` cannot answer this: payloads over
    /// `clipse_core::INLINE_MAX_BYTES` have a `PayloadBody::Blob` body, which
    /// carries no bytes at all, and that covers essentially every screenshot.
    GetPayload {
        id: ClipId,
        format: ClipFormat,
    },
```

Add to `Response`, after the `Clip` variant:

```rust
    /// `None` covers all three ordinary misses: no such clip, no payload in
    /// that format, or a payload past `MAX_PAYLOAD_BYTES`. None of them is an
    /// error — the caller shows a size instead of a picture.
    PayloadBytes(Option<serde_bytes::ByteBuf>),
```

Ensure `ClipFormat` is in the file's `use clipse_core::{...}` list.

- [ ] **Step 5: Write the free function and the handler arm**

In `crates/clipsed/src/daemon.rs`, next to `materialize`:

```rust
/// The bytes of one payload, or `None` for all three ordinary misses: no
/// payload in that format, or one past the preview cap.
///
/// A free function beside `materialize` for the same reason that one is: the
/// interesting behaviour is payload selection, and it is worth testing without
/// standing up a daemon.
fn read_payload(
    store: &Store,
    clip: &Clip,
    format: ClipFormat,
) -> clipse_store::Result<Option<Vec<u8>>> {
    let Some(payload) = clip.payloads.iter().find(|p| p.format == format) else {
        return Ok(None);
    };
    if payload.size > clipse_ipc::MAX_PAYLOAD_BYTES {
        return Ok(None);
    }
    // Inline payloads already carry their bytes; only a blob body needs the
    // content-addressed read.
    match payload.bytes() {
        Some(bytes) => Ok(Some(bytes.to_vec())),
        None => store.read_blob(&payload.digest).map(Some),
    }
}
```

`Payload::bytes()` is the existing accessor at `crates/clipse-core/src/clip.rs:165`.

Then add the handler arm immediately after `Request::GetClip`:

```rust
            Request::GetPayload { id, format } => {
                let read = self
                    .with_store(move |store| match store.get(id)? {
                        Some(clip) => read_payload(store, &clip, format),
                        None => Ok(None),
                    })
                    .await;

                match read {
                    Ok(bytes) => Response::PayloadBytes(bytes.map(serde_bytes::ByteBuf::from)),
                    Err(response) => response,
                }
            }
```

Add `serde_bytes = "0.11"` to `crates/clipsed/Cargo.toml` dependencies.

- [ ] **Step 6: Run the tests to verify they pass**

Run: `cargo test -p clipsed read_payload`
Expected: PASS, 4 tests.

- [ ] **Step 7: Expose it as a Tauri command**

In `apps/clipse-app/src-tauri/src/commands.rs`, after `get_clip`:

```rust
/// Base64 rather than bytes: the only consumer builds a `data:` URL, and Tauri
/// serialises `Vec<u8>` to the webview as a JSON array of numbers — about four
/// characters per byte, for something that then has to be re-encoded anyway.
#[tauri::command]
pub async fn get_payload(
    state: State<'_, Arc<AppState>>,
    id: ClipId,
    format: ClipFormat,
) -> Result<Option<String>, CommandError> {
    match call(&state, Request::GetPayload { id, format }).await? {
        Response::PayloadBytes(bytes) => Ok(bytes.map(|b| BASE64.encode(b.as_ref()))),
        _ => Err(unexpected("get_payload")),
    }
}
```

Add at the top of the same file:

```rust
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
```

and `clipse_core::ClipFormat` to its imports. Add to `apps/clipse-app/src-tauri/Cargo.toml`:

```toml
base64 = "0.22"
```

Register the command in `apps/clipse-app/src-tauri/src/lib.rs`'s `tauri::generate_handler![...]`, on the line after `commands::get_clip,`:

```rust
            commands::get_payload,
```

- [ ] **Step 8: Add the client wrapper**

In `apps/clipse-app/src/lib/tauri-client.ts`, add `ClipFormat` to the type import list, and add to the `api` object next to `getClip`:

```ts
  /** Base64 of one payload, or `null` when the clip, the format, or a size
   * under `MAX_PAYLOAD_BYTES` (24MB) is missing. All three are ordinary
   * answers, not errors. */
  getPayload: (id: string, format: ClipFormat) =>
    call<string | null>("get_payload", { id, format }),
```

- [ ] **Step 9: Verify everything**

Run: `cargo test --workspace && cargo clippy --workspace --all-targets -- -D warnings && cargo fmt --all -- --check`
Expected: clean.

Run: `cd apps/clipse-app && pnpm tsc --noEmit`
Expected: clean.

- [ ] **Step 10: Commit**

```bash
git add crates/clipse-ipc crates/clipsed apps/clipse-app/src-tauri apps/clipse-app/src/lib/tauri-client.ts Cargo.lock
git commit --no-gpg-sign -m "Let a UI ask for the bytes of one payload

GetClip returns a Clip, and any payload over INLINE_MAX_BYTES has a Blob
body carrying no bytes -- which is nearly every screenshot. A detail panel
built on GetClip alone could show a size where the picture should be.

Capped at 24MB, and the cap is about previewing rather than storing: a
400MB file copy is a fine clip, just not one to pull into a webview.
serde_bytes keeps the response a MessagePack bin blob at 1:1, so the cap is
a product decision and not a side effect of integer-array encoding.

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>"
```

---

### Task 4: The ASCII identity

**Files:**
- Create: `apps/clipse-app/src/lib/ascii-logotype.ts`
- Create: `apps/clipse-app/src/lib/ascii-logotype.test.ts`
- Create: `apps/clipse-app/src/components/ascii-logo.tsx`
- Create: `apps/clipse-app/src/components/ascii-logo.module.css`
- Modify: `apps/clipse-app/src/lib/eclipse-ascii.ts` (remove `ECLIPSE_MARK`, fix a stale comment)
- Modify: `apps/clipse-app/src/components/empty-state.tsx`
- Modify: `apps/clipse-app/src/components/daemon-offline-state.tsx`
- Modify: `apps/clipse-app/src/pages/history-window.tsx:2,82`
- Delete: `apps/clipse-app/src/components/ascii-mark.tsx`, `ascii-mark.module.css`, `eclipse-mark.tsx`, `eclipse-mark.module.css`

**Interfaces:**
- Consumes: nothing.
- Produces: `ECLIPSE_MARK: readonly string[]`, `CLIPSE_WORDMARK: readonly string[]`, and `<AsciiLogo variant="mark" | "wordmark" | "lockup" cell={number} />` where `cell` is the character cell height in px.

- [ ] **Step 1: Write the failing test**

Create `apps/clipse-app/src/lib/ascii-logotype.test.ts`:

```ts
import { describe, expect, it } from "vitest";

import { CLIPSE_WORDMARK, ECLIPSE_MARK } from "./ascii-logotype";

/** A character grid is only a drawing if every row is the same width. A short
 * row does not fail loudly — it leans the logo a fraction of a character, which
 * a human eye forgives for weeks and a test catches immediately. */
describe.each([
  ["ECLIPSE_MARK", ECLIPSE_MARK],
  ["CLIPSE_WORDMARK", CLIPSE_WORDMARK],
])("%s", (_name, grid) => {
  it("has rows", () => {
    expect(grid.length).toBeGreaterThan(0);
  });

  it("is rectangular", () => {
    const widths = new Set(grid.map((row) => row.length));
    expect([...widths]).toHaveLength(1);
  });

  it("carries no tab or newline, which would break the grid", () => {
    for (const row of grid) {
      expect(row).not.toMatch(/[\t\n\r]/);
    }
  });
});

describe("CLIPSE_WORDMARK", () => {
  it("is wider than it is tall, as a logotype for six letters must be", () => {
    expect(CLIPSE_WORDMARK[0].length).toBeGreaterThan(CLIPSE_WORDMARK.length * 3);
  });
});
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cd apps/clipse-app && pnpm vitest run src/lib/ascii-logotype.test.ts`
Expected: FAIL — cannot resolve `./ascii-logotype`.

- [ ] **Step 3: Create the logotype module**

Create `apps/clipse-app/src/lib/ascii-logotype.ts`:

```ts
/**
 * The identity, drawn in characters.
 *
 * Fixed grids, not computed ones: `eclipse-ascii.ts` renders the *field* that
 * animates, and it may legitimately differ frame to frame. A logo may not. It
 * has to be the same drawing in the spine, in an empty state and in the
 * colophon, including in the places that never animate.
 *
 * Every row in a grid is the same length. `ascii-logotype.test.ts` enforces
 * that, because a row one character short leans the whole mark.
 */

/** The eclipse: a disc with the moon taking a bite out of it. Nine rows is the
 * smallest that reads as two overlapping circles rather than as texture. */
export const ECLIPSE_MARK: readonly string[] = [
  "  ,:;=!*!=;:,  ",
  " ;=*#      #*= ",
  ":*#          #*",
  "=*            *",
  "!#            #",
  "=*            *",
  ":*#          #*",
  " ;=*#      #*= ",
  "  ,:;=!*!=;:,  ",
];

/** `CLIPSE` as block characters. Six rows, so it sets against the mark's nine
 * without either one looking like an accident of the other. */
export const CLIPSE_WORDMARK: readonly string[] = [
  " ####  #      ###  ####   ####  #### ",
  "#      #       #   #   # #      #    ",
  "#      #       #   #   # #      #    ",
  "#      #       #   ####   ###   ###  ",
  "#      #       #   #         #  #    ",
  " ####  #####  ###  #     ####   #### ",
];
```

Note the wordmark rows above are a starting grid, not a finished drawing. Before moving on, set it in DM Mono at 8px and 16px cell height and adjust the letterforms until the counters in `C`, `P` and `S` stay open at the smaller size. Keep every row exactly 37 characters; the test will catch it if you do not.

- [ ] **Step 4: Run the test to verify it passes**

Run: `cd apps/clipse-app && pnpm vitest run src/lib/ascii-logotype.test.ts`
Expected: PASS, 8 tests.

- [ ] **Step 5: Remove the duplicate mark from `eclipse-ascii.ts`**

Delete the `ECLIPSE_MARK` export (the whole block from its doc comment at line 132 to the end of the file). Two sources of truth for one logo is how the two drift apart.

While in the file, fix the stale reference in the comment at line 20 of `eclipse-canvas.tsx` — it says "IBM Plex Mono advances 0.6em"; the mono face is DM Mono (`tokens.css:92`). Correct the face name and leave the number, which is measured behaviour rather than a claim about a specific font.

- [ ] **Step 6: Create the component**

Create `apps/clipse-app/src/components/ascii-logo.tsx`:

```tsx
import { CLIPSE_WORDMARK, ECLIPSE_MARK } from "../lib/ascii-logotype";
import styles from "./ascii-logo.module.css";

/** Below this many pixels per character cell the nine-row eclipse stops being
 * two circles and becomes a smudge. This is the constraint that killed the
 * previous attempt at an ASCII mark, and the answer is not a cleverer grid —
 * it is not asking the grid for something it cannot give. A mark that must sit
 * at 16px (a tray icon, an OS icon) stays an SVG. */
const MIN_CELL_PX = 4.5;

export interface AsciiLogoProps {
  variant?: "mark" | "wordmark" | "lockup";
  /** Height of one character cell in px. The grid scales from this alone, so
   * the drawing is identical at every size. */
  cell?: number;
  className?: string;
}

export function AsciiLogo({ variant = "mark", cell = 6, className }: AsciiLogoProps) {
  const size = Math.max(MIN_CELL_PX, cell);
  const rows =
    variant === "mark"
      ? ECLIPSE_MARK
      : variant === "wordmark"
        ? CLIPSE_WORDMARK
        : [...ECLIPSE_MARK, "", ...CLIPSE_WORDMARK];

  return (
    <pre
      className={[styles.logo, className].filter(Boolean).join(" ")}
      style={{ fontSize: `${size}px` }}
      role="img"
      aria-label="Clipse"
    >
      {rows.join("\n")}
    </pre>
  );
}
```

Create `apps/clipse-app/src/components/ascii-logo.module.css`:

```css
/* The grid is the drawing, so nothing may nudge a character off it:
 * `line-height: 1` and zero tracking keep the cells square to each other, and
 * ligatures are off because DM Mono would happily fuse the `=*` pairs in the
 * corona into a single glyph and bend the circle. */
.logo {
  margin: 0;
  font-family: var(--font-mono);
  font-weight: 400;
  line-height: 1;
  letter-spacing: 0;
  font-variant-ligatures: none;
  white-space: pre;
  color: currentColor;
  user-select: none;
}
```

- [ ] **Step 7: Replace the SVG marks in the two states that use them**

In `apps/clipse-app/src/components/empty-state.tsx`, replace the `EclipseMark` import and its usage. The loading variant keeps its motion by using the existing computed canvas; the still variant uses the fixed mark:

```tsx
import { EclipseCanvas } from "./eclipse-canvas";
import { AsciiLogo } from "./ascii-logo";
```

and in the returned JSX, replace `<EclipseMark size={64} animated={animated} className={styles.mark} />` with:

```tsx
      {animated ? (
        <div className={styles.field}>
          <EclipseCanvas phase={0.5} />
        </div>
      ) : (
        <AsciiLogo variant="mark" cell={7} className={styles.mark} />
      )}
```

Add to `empty-state.module.css`:

```css
/* The animated field needs a box to fit itself into; EclipseCanvas measures
 * its parent. */
.field {
  width: 100%;
  max-width: 22rem;
  aspect-ratio: 5 / 2;
  color: var(--color-ink-muted);
}
```

In `apps/clipse-app/src/components/daemon-offline-state.tsx`, replace the `EclipseMark` import with `AsciiLogo` and `<EclipseMark size={64} variant="mono" className={styles.mark} />` with:

```tsx
      <AsciiLogo variant="mark" cell={7} className={styles.mark} />
```

A machine that cannot reach its daemon gets the still mark, not the animated field: nothing is in progress, and an animation would say otherwise.

- [ ] **Step 8: Point the history masthead at the new component**

In `apps/clipse-app/src/pages/history-window.tsx`, change line 2's import to `import { AsciiLogo } from "../components/ascii-logo";` and line 82's `<AsciiMark />` to `<AsciiLogo variant="mark" cell={5} />`.

This is temporary — Task 6 moves it into the spine — but it keeps the app working and the suite green at the end of this task, which is the point of the boundary.

- [ ] **Step 9: Delete the SVG marks**

```bash
git rm apps/clipse-app/src/components/ascii-mark.tsx apps/clipse-app/src/components/ascii-mark.module.css apps/clipse-app/src/components/eclipse-mark.tsx apps/clipse-app/src/components/eclipse-mark.module.css
```

- [ ] **Step 10: Verify**

Run: `cd apps/clipse-app && pnpm vitest run && pnpm tsc --noEmit`
Expected: all vitest green (56 + 8 new), tsc clean.

Run: `cd apps/clipse-app && pnpm exec playwright test`
Expected: 9 existing tests still pass. If an onboarding test asserted on the SVG mark, update the assertion to read the `<pre>` text rather than deleting the test.

- [ ] **Step 11: Commit**

```bash
git add -A apps/clipse-app/src
git commit --no-gpg-sign -m "Draw the identity in characters, and delete the SVG that claimed to

ascii-mark.tsx was SVG geometry under an ASCII name, and a working
character-grid eclipse sat unused in eclipse-ascii.ts as dead code. Both
marks are now one grid, in one module, with a test that every row is the
same width -- the failure that leans a logo a fraction of a character and
survives visual review for weeks.

The old component's comment was right that nine rows cannot survive 16px.
The answer is not a cleverer grid: the mark is never asked to render below
4.5px per cell, and a tray or OS icon stays SVG, because an operating
system will not take a character grid.

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>"
```

---

### Task 5: The frameless window

**Files:**
- Modify: `apps/clipse-app/src-tauri/tauri.conf.json:14-23`
- Modify: `apps/clipse-app/src-tauri/capabilities/default.json`
- Create: `apps/clipse-app/src/lib/window-frame.ts`
- Create: `apps/clipse-app/src/lib/window-frame.test.ts`
- Create: `apps/clipse-app/src/components/window-controls.tsx`
- Create: `apps/clipse-app/src/components/window-controls.module.css`
- Create: `apps/clipse-app/src/components/resize-handles.tsx`
- Create: `apps/clipse-app/src/components/resize-handles.module.css`
- Modify: `apps/clipse-app/src/pages/history-window.tsx`, `history-window.module.css`

**Interfaces:**
- Consumes: nothing.
- Produces: `RESIZE_EDGES: readonly ResizeEdge[]` and `resizeDirection(edge: ResizeEdge): string` from `lib/window-frame.ts`; `<WindowControls />`; `<ResizeHandles />`.

- [ ] **Step 1: Write the failing test**

Create `apps/clipse-app/src/lib/window-frame.test.ts`:

```ts
import { describe, expect, it } from "vitest";

import { RESIZE_EDGES, resizeDirection } from "./window-frame";

describe("resize edges", () => {
  it("covers all four sides and all four corners", () => {
    expect(RESIZE_EDGES).toHaveLength(8);
    expect(new Set(RESIZE_EDGES).size).toBe(8);
  });

  /** Tauri's startResizeDragging takes PascalCase direction names. A typo
   * here does not throw — the drag simply does nothing, on one edge only,
   * which is exactly the bug nobody finds by hand. */
  it("maps every edge to a Tauri direction name", () => {
    const expected = new Set([
      "North",
      "South",
      "East",
      "West",
      "NorthEast",
      "NorthWest",
      "SouthEast",
      "SouthWest",
    ]);
    const got = new Set(RESIZE_EDGES.map(resizeDirection));
    expect(got).toEqual(expected);
  });
});
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cd apps/clipse-app && pnpm vitest run src/lib/window-frame.test.ts`
Expected: FAIL — cannot resolve `./window-frame`.

- [ ] **Step 3: Create the module**

Create `apps/clipse-app/src/lib/window-frame.ts`:

```ts
/**
 * The parts of a frameless window that Win32 stops providing for free.
 *
 * `decorations: false` removes the OS title bar, and with it the resize border
 * and the double-click-to-maximize target. Both have to be put back by hand,
 * and the mapping below is the part that fails silently if it is wrong: a
 * misspelled direction does not throw, it just makes one edge dead.
 */

export type ResizeEdge = "n" | "s" | "e" | "w" | "ne" | "nw" | "se" | "sw";

export const RESIZE_EDGES: readonly ResizeEdge[] = [
  "n",
  "s",
  "e",
  "w",
  "ne",
  "nw",
  "se",
  "sw",
];

const DIRECTIONS: Record<ResizeEdge, string> = {
  n: "North",
  s: "South",
  e: "East",
  w: "West",
  ne: "NorthEast",
  nw: "NorthWest",
  se: "SouthEast",
  sw: "SouthWest",
};

export function resizeDirection(edge: ResizeEdge): string {
  return DIRECTIONS[edge];
}
```

- [ ] **Step 4: Run the test to verify it passes**

Run: `cd apps/clipse-app && pnpm vitest run src/lib/window-frame.test.ts`
Expected: PASS, 2 tests.

- [ ] **Step 5: Make the window frameless**

In `apps/clipse-app/src-tauri/tauri.conf.json`, replace the `main` window object with:

```json
      {
        "label": "main",
        "title": "Clipse",
        "width": 960,
        "height": 640,
        "minWidth": 720,
        "minHeight": 420,
        "visible": true,
        "decorations": false
      }
```

`minWidth` rises from 640 to 720, deliberately *below* the 760px point where Task 7's peek panel switches from a column to an overlay — so the overlay is a state a person can actually reach by resizing, not code only a bug could run.

`decorations: false` applies on all three platforms, which is the consistency that was chosen over native feel. On macOS it takes the traffic lights with it. **Keep that reversible without a second masthead:** the only change a per-platform split would need is `"decorations": false` here plus `"titleBarStyle": "Overlay"` and a `trafficLightPosition` on macOS, with `<WindowControls />` rendered behind a platform check. Do not build that now, and do not write a second layout for it — just don't let anything else in the layout assume the controls are always present. Concretely: the search row must not depend on `<WindowControls />` for its spacing, so give the row its own right padding rather than letting the controls provide it.

Add the window permissions to `apps/clipse-app/src-tauri/capabilities/default.json`:

```json
  "permissions": [
    "core:default",
    "opener:default",
    "core:window:allow-minimize",
    "core:window:allow-unminimize",
    "core:window:allow-toggle-maximize",
    "core:window:allow-close",
    "core:window:allow-start-dragging",
    "core:window:allow-start-resize-dragging"
  ]
```

- [ ] **Step 6: Build the window controls**

Create `apps/clipse-app/src/components/window-controls.tsx`:

```tsx
import { getCurrentWindow } from "@tauri-apps/api/window";

import styles from "./window-controls.module.css";

/**
 * Minimise, maximise, close — as three mono characters, with no bar around
 * them.
 *
 * That absence is the design. The native title bar was removed because it sat
 * as a second header above the masthead; drawing our own would be the same
 * problem in our own colours. These sit on the masthead's baseline and the
 * masthead is the frame.
 */
export function WindowControls() {
  const win = getCurrentWindow();

  return (
    <div className={styles.controls}>
      <button
        type="button"
        className={styles.control}
        aria-label="Minimise"
        onClick={() => void win.minimize()}
      >
        –
      </button>
      <button
        type="button"
        className={styles.control}
        aria-label="Maximise"
        onClick={() => void win.toggleMaximize()}
      >
        ▢
      </button>
      <button
        type="button"
        className={`${styles.control} ${styles.close}`}
        aria-label="Close"
        onClick={() => void win.close()}
      >
        ✕
      </button>
    </div>
  );
}
```

Create `apps/clipse-app/src/components/window-controls.module.css`:

```css
.controls {
  display: flex;
  align-items: center;
  gap: 0.15rem;
  /* Above the drag region, or the buttons become part of it and a click
   * starts a window drag instead of pressing them. */
  position: relative;
  z-index: 1;
}

.control {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  width: 1.75rem;
  height: 1.5rem;
  padding: 0;
  border: 0;
  background: none;
  color: var(--color-ink-muted);
  font-family: var(--font-mono);
  font-size: var(--type-xs);
  line-height: 1;
  cursor: default;
  transition: color var(--duration-fast) var(--ease-out);
}

.control:hover {
  color: var(--color-ink);
}

/* The only red in the frame, and only while it is about to happen. */
.close:hover {
  color: var(--color-accent);
}
```

- [ ] **Step 7: Build the resize handles**

Create `apps/clipse-app/src/components/resize-handles.tsx`:

```tsx
import { getCurrentWindow } from "@tauri-apps/api/window";

import { RESIZE_EDGES, resizeDirection } from "../lib/window-frame";
import styles from "./resize-handles.module.css";

/**
 * Eight invisible grips around the window edge.
 *
 * A frameless Win32 window has no resize border, so without these the window
 * is a fixed size and nothing on screen explains why. Kept as its own
 * component because it is frame plumbing and belongs nowhere near the layout
 * it happens to sit inside.
 */
export function ResizeHandles() {
  const win = getCurrentWindow();

  return (
    <>
      {RESIZE_EDGES.map((edge) => (
        <div
          key={edge}
          className={`${styles.handle} ${styles[edge]}`}
          data-edge={edge}
          onMouseDown={(event) => {
            if (event.button !== 0) return;
            event.preventDefault();
            void win.startResizeDragging(resizeDirection(edge) as never);
          }}
        />
      ))}
    </>
  );
}
```

Create `apps/clipse-app/src/components/resize-handles.module.css`:

```css
/* 8px: the smallest target a person can reliably hit without the grips
 * stealing clicks from the content behind them. */
.handle {
  position: fixed;
  z-index: 10;
}

.n, .s { left: 8px; right: 8px; height: 8px; cursor: ns-resize; }
.e, .w { top: 8px; bottom: 8px; width: 8px; cursor: ew-resize; }
.n { top: 0; }
.s { bottom: 0; }
.e { right: 0; }
.w { left: 0; }

.ne, .nw, .se, .sw { width: 12px; height: 12px; }
.nw { top: 0; left: 0; cursor: nwse-resize; }
.se { bottom: 0; right: 0; cursor: nwse-resize; }
.ne { top: 0; right: 0; cursor: nesw-resize; }
.sw { bottom: 0; left: 0; cursor: nesw-resize; }
```

- [ ] **Step 8: Mount them, and make the header draggable**

In `apps/clipse-app/src/pages/history-window.tsx`: import both components, render `<ResizeHandles />` as the first child of `styles.window`, add `<WindowControls />` inside the header after the settings button, and add `data-tauri-drag-region` to the `<header>` element.

Double-click to maximise comes free: Tauri's drag region handles it on a region marked `data-tauri-drag-region`. Confirm that by hand in Step 10 rather than assuming it.

- [ ] **Step 9: Verify what can be verified from here**

Run: `cd apps/clipse-app && pnpm vitest run && pnpm tsc --noEmit && pnpm exec playwright test`
Expected: green. Run: `cargo clippy --workspace --all-targets -- -D warnings` — clean.

- [ ] **Step 10: Drive it by hand on Windows and record the result**

Run: `cd apps/clipse-app && pnpm tauri dev`

Check, and write the outcome — including anything that does *not* work — into `docs/manual-verification.md` under a new dated heading:

1. No OS title bar above the masthead.
2. Dragging the header moves the window.
3. Double-clicking the header toggles maximise.
4. All eight edges and corners resize.
5. Dragging to the top of the screen snaps to maximise; dragging to a side snaps to half. **This is the one genuinely uncertain item.** If Tauri's drag emulation does not produce Aero Snap, do not paper over it: record the limitation, note that `Win`+arrow still works at the OS level, and leave it.
6. Minimise, maximise and close all work, and closing hides to the tray rather than quitting (`lib.rs:111-119`).
7. A second launch of the built app focuses the first window instead of opening another (Task 1, verifiable only here).

- [ ] **Step 11: Commit**

```bash
git add apps/clipse-app docs/manual-verification.md
git commit --no-gpg-sign -m "Take the OS title bar off the main window

On Windows the native title bar sat directly above the editorial masthead:
two top bars, one of them ours. The window is frameless now and gets no
replacement title bar either -- drawing one would be the same problem in
our own colours. The controls sit on the masthead's baseline, and the
masthead is the frame.

What frameless costs on Win32 is the resize border and the snap
affordance, so eight invisible grips call startResizeDragging and the
direction mapping is unit-tested: a misspelled direction does not throw, it
just makes one edge dead. Snap behaviour is recorded in
docs/manual-verification.md rather than assumed.

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>"
```

---

### Task 6: The spine and the hanging edge

**Files:**
- Create: `apps/clipse-app/src/components/spine.tsx`, `spine.module.css`
- Create: `apps/clipse-app/src/components/kind-glyph.tsx`
- Modify: `apps/clipse-app/src/pages/history-window.tsx`, `history-window.module.css`
- Modify: `apps/clipse-app/src/components/clip-row.tsx`, `clip-row.module.css`
- Modify: `apps/clipse-app/src/components/search-box.module.css`, `type-filter-tabs.module.css`
- Modify: `apps/clipse-app/e2e/history.spec.ts`

**Interfaces:**
- Consumes: `<AsciiLogo />` (Task 4), `<WindowControls />` (Task 5), `DaemonStatus.secrets_refused` (Task 2).
- Produces: `<Spine clipCount secretsRefused paused loadingMore peers onOpenSettings settingsActive />`; `<KindGlyph clip />`.

`KindIcon` is left alone: the popup uses it, and the popup is explicitly out of scope. `KindGlyph` is the history row's mono equivalent.

- [ ] **Step 1: Write the failing test**

Add to `apps/clipse-app/e2e/history.spec.ts`:

```ts
test("the spine carries the identity as text, not as an image", async ({ page }) => {
  // The whole point of the ASCII identity is that it is characters. An <img>
  // or an inline <svg> would look identical in a screenshot and be a
  // regression, so assert on the text content.
  const logo = page.getByRole("img", { name: "Clipse" });
  await expect(logo).toBeVisible();
  await expect(logo).toContainText("#");

  await expect(page.getByRole("button", { name: "Minimise" })).toBeVisible();
  await expect(page.getByRole("button", { name: "Close" })).toBeVisible();
});
```

There is no per-test setup call to make: `history.spec.ts` already has a `test.beforeEach` that installs the Tauri stub with `FIXTURE_CLIPS` / `FIXTURE_STATUS` / `FIXTURE_SETTINGS` and navigates to `/`. Every test in this file starts on a populated history window.

Because the fixture has clips, the empty state never mounts, so `getByRole("img", { name: "Clipse" })` matches only the spine and Playwright's strict mode is satisfied. If a later change makes a second logo visible at once, give the spine's `AsciiLogo` a distinguishing `aria-label` rather than loosening the locator.

- [ ] **Step 2: Run the test to verify it fails**

Run: `cd apps/clipse-app && pnpm exec playwright test history.spec.ts -g "spine"`
Expected: FAIL — the ASCII logo is present from Task 4, but the assertion for both window controls in one masthead is not satisfied until the layout lands, or the strict-mode locator matches more than one "Clipse" image.

- [ ] **Step 3: Build the spine**

Create `apps/clipse-app/src/components/spine.tsx`:

```tsx
import { AsciiLogo } from "./ascii-logo";
import { SettingsIcon } from "./icons";
import styles from "./spine.module.css";

export interface SpineProps {
  clipCount: number;
  /** Captures dropped for looking like a secret, since the daemon started. */
  secretsRefused: number;
  paused: boolean;
  loadingMore: boolean;
  peersOnline: number;
  peersTotal: number;
  settingsActive: boolean;
  onOpenSettings: () => void;
  /** The count element, so the window can animate it on change. */
  countRef?: React.Ref<HTMLSpanElement>;
}

/**
 * The left rail, and the window's entire chrome.
 *
 * It replaces a header, a toolbar's right-hand controls and a footer. The
 * onboarding put the step numeral on a rotated spine; this is that spine made
 * permanent, and the composition it forces — everything structural down one
 * narrow edge, the content given the whole rest of the window — is the reason
 * there is no title bar to reconcile.
 *
 * Its empty middle is the window's drag region: space that exists for the
 * composition anyway, so the frame costs no vertical room.
 */
export function Spine({
  clipCount,
  secretsRefused,
  paused,
  loadingMore,
  peersOnline,
  peersTotal,
  settingsActive,
  onOpenSettings,
  countRef,
}: SpineProps) {
  return (
    <aside className={styles.spine}>
      <AsciiLogo variant="mark" cell={5.5} className={styles.mark} />
      <span className={styles.wordmark} aria-hidden="true">
        CLIPSE
      </span>

      <div className={styles.drag} data-tauri-drag-region />

      <div className={styles.meter}>
        <span className={styles.count} data-numeric ref={countRef}>
          {clipCount}
        </span>
        <span className={styles.label}>{clipCount === 1 ? "clip" : "clips"}</span>
      </div>

      {/* The promise made visible. A count and nothing else: the daemon does
        * not record what it refused, so there is nothing else to show. Set in
        * normal ink however high it goes -- the accent belongs to the moment
        * of an increment, which the window flashes, and to the hovered close
        * control. Zero is shown rather than hidden, because "nothing has been
        * refused" is also information. */}
      <div className={styles.refused} title="Captures refused for looking like a secret">
        <span className={styles.refusedCount} data-numeric>
          {secretsRefused}
        </span>
        <span className={styles.label}>refused</span>
      </div>

      <div className={styles.state}>
        {paused && <span className={styles.paused}>paused</span>}
        {loadingMore && <span className={styles.loading}>loading</span>}
      </div>

      <div className={styles.foot}>
        <span className={styles.peers} title={`${peersOnline} of ${peersTotal} devices online`}>
          {Array.from({ length: peersTotal }, (_, i) => (
            <span key={i} className={i < peersOnline ? styles.dotOn : styles.dotOff} />
          ))}
        </span>
        <button
          type="button"
          className={settingsActive ? `${styles.settings} ${styles.on}` : styles.settings}
          aria-label="Settings"
          aria-pressed={settingsActive}
          onClick={onOpenSettings}
        >
          <SettingsIcon size={15} />
        </button>
      </div>
    </aside>
  );
}
```

Create `apps/clipse-app/src/components/spine.module.css`:

```css
.spine {
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: 0.85rem;
  width: 4.5rem;
  padding: 1rem 0 0.75rem;
  /* A rule, not a border: structure in this product comes from hairlines and
   * space. */
  border-right: 1px solid var(--color-rule);
  background: var(--color-surface-sunken);
  flex-shrink: 0;
}

.mark {
  color: var(--color-ink-muted);
}

/* Set up the edge like a book spine, which is where the device comes from. */
.wordmark {
  writing-mode: vertical-rl;
  font-family: var(--font-display);
  font-weight: var(--weight-display);
  font-size: var(--type-xs);
  letter-spacing: var(--tracking-label);
  text-transform: uppercase;
  color: var(--color-ink-secondary);
}

/* Claims the slack, and is the drag region precisely because it is slack. */
.drag {
  flex: 1;
  align-self: stretch;
  min-height: 1.5rem;
}

.meter,
.refused,
.state,
.foot {
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: 0.15rem;
}

/* The one big number in the product: it answers "did that copy land". */
.count {
  font-family: var(--font-display);
  font-weight: var(--weight-display);
  font-size: var(--type-lg);
  line-height: var(--leading-crushed);
  letter-spacing: var(--tracking-crushed);
  color: var(--color-ink);
  font-variant-numeric: tabular-nums;
}

.refusedCount {
  font-family: var(--font-mono);
  font-size: var(--type-sm);
  color: var(--color-ink-secondary);
  font-variant-numeric: tabular-nums;
}

.label {
  font-family: var(--font-mono);
  font-size: var(--type-2xs);
  text-transform: uppercase;
  letter-spacing: var(--tracking-label);
  color: var(--color-ink-muted);
  text-align: center;
}

.state {
  min-height: 1rem;
  font-family: var(--font-mono);
  font-size: var(--type-2xs);
  text-transform: uppercase;
  letter-spacing: var(--tracking-wide);
}

.paused {
  color: var(--color-accent-ink);
}

.loading {
  color: var(--color-ink-muted);
}

.foot {
  gap: 0.6rem;
  padding-top: 0.25rem;
}

.peers {
  display: flex;
  gap: 3px;
}

.dotOn,
.dotOff {
  width: 4px;
  height: 4px;
  border-radius: var(--radius-pill);
}

.dotOn {
  background: var(--color-success);
}

.dotOff {
  background: var(--color-rule-strong);
}

.settings {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  width: 1.9rem;
  height: 1.9rem;
  border: 0;
  border-radius: var(--radius-md);
  background: none;
  color: var(--color-ink-muted);
  cursor: default;
  transition: color var(--duration-fast) var(--ease-out);
}

.settings:hover,
.settings.on {
  color: var(--color-ink);
}
```

- [ ] **Step 4: Build the mono kind glyph**

Create `apps/clipse-app/src/components/kind-glyph.tsx`:

```tsx
import { looksLikeLink } from "../lib/clip-content";
import type { Clip } from "../types/ipc";

/**
 * A clip's kind, as one character.
 *
 * A history row is a line of set text, and an SVG icon in it is a foreign
 * object sitting on the baseline. The popup keeps `KindIcon` — it is a
 * different surface, summoned mid-task, and is out of scope here.
 */
export function KindGlyph({ clip }: { clip: Clip }) {
  const glyph =
    clip.kind === "image" ? "▤" : clip.kind === "files" ? "▧" : looksLikeLink(clip) ? "↗" : "—";
  return (
    <span aria-hidden="true" data-kind={clip.kind}>
      {glyph}
    </span>
  );
}
```

- [ ] **Step 5: Rebuild the history window around the spine**

Rewrite `apps/clipse-app/src/pages/history-window.tsx`'s returned JSX. The handlers, the entry animation and the count animation are unchanged; the structure becomes:

```tsx
  return (
    <div className={styles.window} ref={root}>
      <ResizeHandles />

      <Spine
        clipCount={history.clips.length}
        secretsRefused={status?.secrets_refused ?? 0}
        paused={status?.paused ?? false}
        loadingMore={history.loadingMore}
        peersOnline={status?.peers_online ?? 0}
        peersTotal={status?.peers_total ?? 0}
        settingsActive={false}
        onOpenSettings={() => setView("settings")}
        countRef={countRef}
      />

      <div className={styles.main}>
        {/* No title bar and no toolbar: one row carries search, the filters
          * and the window controls, and the space around it is the drag
          * region. */}
        <div className={styles.top} data-tauri-drag-region data-enter>
          <SearchBox
            value={history.searchText}
            onChange={history.setSearchText}
            placeholder="Search everything you've copied"
          />
          <TypeFilterTabs value={history.typeFilter} onChange={history.setTypeFilter} />
          <button
            type="button"
            className={history.pinnedOnly ? `${styles.pinToggle} ${styles.active}` : styles.pinToggle}
            aria-label="Show pinned only"
            aria-pressed={history.pinnedOnly}
            onClick={() => history.setPinnedOnly(!history.pinnedOnly)}
          >
            {history.pinnedOnly ? <PinFilledIcon size={15} /> : <PinIcon size={15} />}
          </button>
          <WindowControls />
        </div>

        {status?.capture_mode && status.capture_mode !== "Automatic" && (
          <div className={styles.banner}>
            <CaptureModeBanner captureMode={status.capture_mode} />
          </div>
        )}

        {!history.offline && history.errorMessage && (
          <div className={styles.banner}>
            <p className={styles.errorBanner}>{history.errorMessage}</p>
          </div>
        )}

        <main className={styles.body}>{/* unchanged conditional list body */}</main>
      </div>
    </div>
  );
```

Delete the old `<header>`, the old `<div className={styles.toolbar}>` and the whole `<footer>` block — `paused` and `loadingMore` now live in the spine, and an empty status bar is a row of chrome earning nothing.

Rewrite `history-window.module.css`'s structural rules:

```css
.window {
  display: grid;
  grid-template-columns: auto 1fr;
  height: 100vh;
  background: var(--color-canvas);
  overflow: hidden;
}

.main {
  display: flex;
  flex-direction: column;
  min-width: 0;
  min-height: 0;
}

/* The search row is the masthead. Generous padding above it is what makes a
 * drag region out of space the composition wanted anyway. */
.top {
  display: flex;
  align-items: center;
  gap: 0.75rem;
  padding: 1rem 1rem 0.85rem 1.5rem;
  border-bottom: 1px solid var(--color-rule);
  flex-shrink: 0;
}
```

Keep `.banner`, `.errorBanner` and `.body` as they are; delete `.header`, `.brand`, `.title`, `.meter`, `.count`, `.countLabel`, `.toolbar`, `.footer` and `.paused`. Rename `.iconButton` to `.pinToggle` and keep its rules, dropping the border and background so it reads as a glyph rather than a button:

```css
.pinToggle {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  width: 1.9rem;
  height: 1.9rem;
  flex-shrink: 0;
  border: 0;
  background: none;
  color: var(--color-ink-muted);
  cursor: default;
  transition: color var(--duration-fast) var(--ease-out);
}

.pinToggle:hover { color: var(--color-ink); }
.pinToggle.active { color: var(--color-accent-ink); }
```

- [ ] **Step 6: Unbox the search field and the filter tabs**

In `search-box.module.css`, remove the wrapper's border, background and radius, and replace them with a single bottom hairline; raise the input to `font-size: var(--type-md)`; keep the search icon but set it to `--color-ink-muted`.

In `type-filter-tabs.module.css`, remove the pill background and border from each tab; set the tabs to `font-family: var(--font-mono)`, `font-size: var(--type-2xs)`, `text-transform: uppercase`, `letter-spacing: var(--tracking-label)`; mark the active tab with `border-bottom: 1px solid var(--color-accent)` and `color: var(--color-ink)` instead of a filled background.

- [ ] **Step 7: Re-grid the row and hang the pin outside the content column**

In `clip-row.tsx`, replace `<KindIcon clip={clip} size={15} />` with `<KindGlyph clip={clip} />`, and move the pinned indicator to the front of the row as a margin tick:

```tsx
      {clip.pinned && <span className={styles.pinTick} aria-hidden="true" />}
```

Remove the old trailing `{clip.pinned && !onTogglePin && <PinFilledIcon .../>}` line.

In `clip-row.module.css`, set the row to an explicit grid and give the tick a negative offset so it hangs outside the content column:

```css
.row {
  display: grid;
  grid-template-columns: 1.5rem 1fr 6rem 4rem auto;
  align-items: center;
  gap: 0.75rem;
  padding-left: 1.5rem;
  padding-right: 0.75rem;
  border-bottom: 1px solid var(--color-rule);
}

.kind {
  font-family: var(--font-mono);
  font-size: var(--type-sm);
  color: var(--color-ink-muted);
  text-align: center;
}

/* Outside the content column, in the margin the padding-left reserves. The
 * overhang is the point: a pinned row breaks the left edge of the text block,
 * which is visible at a glance in a way an inline icon is not. */
.pinTick {
  position: absolute;
  left: 0.55rem;
  width: 2px;
  height: 1.1rem;
  background: var(--color-accent);
}
```

The row is already absolutely positioned by the virtual list (`clip-list.tsx:77` sets `top`/`height`), so `.pinTick`'s `position: absolute` resolves against the row. Confirm the row itself has `position: absolute` in the existing CSS; if it does not, add `position: relative` to `.row`.

Move `.time` and `.device` out of the `.meta` cluster into their own grid cells, both `font-family: var(--font-mono)`, `font-size: var(--type-2xs)`, `color: var(--color-ink-muted)`, and delete the `.dot` separator from both the CSS and the JSX — a grid does not need a middot to separate its columns. Keep `.meta` only if the popup's compact row still needs it; if `compact` is the only consumer, guard the two-cell layout behind `:not(.compact)` so the popup is untouched.

- [ ] **Step 8: Run the tests**

Run: `cd apps/clipse-app && pnpm vitest run && pnpm tsc --noEmit`
Expected: green.

Run: `cd apps/clipse-app && pnpm exec playwright test`
Expected: green, including the new spine test.

One existing test needs attention: `the masthead reports the clip count` asserts on the count and the word `clips` as two separate elements. The spine keeps both, so the assertions still hold — but its comment says the count "lives in the masthead now", which stops being true. Update the comment to say the spine. Any other test that located something in the old header or footer gets its **locator** updated and its assertion left alone; a test that has to weaken what it claims in order to pass is reporting a regression, not a stale selector.

- [ ] **Step 9: Commit**

```bash
git add apps/clipse-app
git commit --no-gpg-sign -m "Put the history window on a spine

The window was a header, a toolbar, a list and a footer: competent and
anonymous, and nothing like the onboarding it opens with. The onboarding
set its step numeral on a rotated spine; this is that spine made permanent.
It absorbs the header, the footer and the window controls, which is what
lets a frameless window have no title bar to reconcile -- and its empty
middle is the drag region, so the frame costs no vertical space.

Rows are set as text now: the kind is one mono character rather than an
SVG on the baseline, and a pinned row breaks the left margin with a tick
outside the content column, which reads at a glance where an inline icon
did not.

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>"
```

---

### Task 7: The peek panel

**Files:**
- Modify: `apps/clipse-app/src/lib/clip-content.ts`
- Create: `apps/clipse-app/src/lib/clip-content.test.ts` (or extend it if it exists)
- Create: `apps/clipse-app/src/hooks/use-clip-payload.ts`
- Create: `apps/clipse-app/src/components/peek-panel.tsx`, `peek-panel.module.css`
- Modify: `apps/clipse-app/src/pages/history-window.tsx`, `history-window.module.css`
- Modify: `apps/clipse-app/e2e/fixtures/tauri-stub.ts`
- Modify: `apps/clipse-app/e2e/history.spec.ts`

**Interfaces:**
- Consumes: `api.getPayload` (Task 3), the spine grid (Task 6).
- Produces: `payloadDataUrl(format: ClipFormat, base64: string): string`, `mimeForFormat(format: ClipFormat): string | null`, `useClipPayload(clip: Clip | null)`, `<PeekPanel clip onClose />`.

- [ ] **Step 1: Write the failing test**

Create `apps/clipse-app/src/lib/clip-content.test.ts`:

```ts
import { describe, expect, it } from "vitest";

import { mimeForFormat, payloadDataUrl } from "./clip-content";

describe("mimeForFormat", () => {
  it("maps the three image formats", () => {
    expect(mimeForFormat("Png")).toBe("image/png");
    expect(mimeForFormat("Jpeg")).toBe("image/jpeg");
    expect(mimeForFormat("Svg")).toBe("image/svg+xml");
  });

  it("returns null for formats that are not images", () => {
    expect(mimeForFormat("Text")).toBeNull();
    expect(mimeForFormat("FileList")).toBeNull();
    expect(mimeForFormat({ Other: "text/csv" })).toBeNull();
  });
});

describe("payloadDataUrl", () => {
  it("builds a data URL from base64 the daemon returned", () => {
    expect(payloadDataUrl("Png", "aGk=")).toBe("data:image/png;base64,aGk=");
  });

  it("refuses to build one for a non-image format", () => {
    // A data: URL for something the panel will not render is a footgun
    // waiting for the next person to point an <img> at it.
    expect(payloadDataUrl("Text", "aGk=")).toBeNull();
  });
});
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cd apps/clipse-app && pnpm vitest run src/lib/clip-content.test.ts`
Expected: FAIL — `mimeForFormat` and `payloadDataUrl` are not exported.

- [ ] **Step 3: Add the two helpers**

In `apps/clipse-app/src/lib/clip-content.ts`, export the existing `IMAGE_MIME` lookup through a function and add the URL builder:

```ts
/** The MIME type for a format the panel can render as an image, or `null`
 * for everything else. */
export function mimeForFormat(format: ClipFormat): string | null {
  const label = typeof format === "string" ? format : "Other";
  return IMAGE_MIME[label] ?? null;
}

/** A `data:` URL from base64 the daemon returned for a blob-backed payload.
 * `null` when the format is not one the panel renders as an image. */
export function payloadDataUrl(format: ClipFormat, base64: string): string | null {
  const mime = mimeForFormat(format);
  return mime ? `data:${mime};base64,${base64}` : null;
}
```

- [ ] **Step 4: Run the test to verify it passes**

Run: `cd apps/clipse-app && pnpm vitest run src/lib/clip-content.test.ts`
Expected: PASS, 4 tests.

- [ ] **Step 5: Add the fetching hook**

Create `apps/clipse-app/src/hooks/use-clip-payload.ts`:

```ts
import { useEffect, useState } from "react";

import { findPayload, getClipImageDataUrl, payloadDataUrl } from "../lib/clip-content";
import { api } from "../lib/tauri-client";
import type { Clip } from "../types/ipc";

export interface ClipPayload {
  /** A `data:` URL for the clip's image, once there is one. */
  imageUrl: string | null;
  loading: boolean;
  /** True when the daemon declined: the payload is past the 24MB preview cap.
   * The panel shows the size instead, which is the honest answer. */
  tooLarge: boolean;
}

/**
 * The bytes behind a clip, fetched only when something is actually looking.
 *
 * Inline payloads (under 64KB) already travel with the clip and need no
 * request at all. Anything larger has a `Blob` body carrying no bytes, so it
 * takes `get_payload` — which is the whole reason that request exists.
 */
export function useClipPayload(clip: Clip | null): ClipPayload {
  const [state, setState] = useState<ClipPayload>({
    imageUrl: null,
    loading: false,
    tooLarge: false,
  });

  useEffect(() => {
    if (!clip || clip.kind !== "image") {
      setState({ imageUrl: null, loading: false, tooLarge: false });
      return;
    }

    const inline = getClipImageDataUrl(clip);
    if (inline) {
      setState({ imageUrl: inline, loading: false, tooLarge: false });
      return;
    }

    const payload =
      findPayload(clip, "Png") ?? findPayload(clip, "Jpeg") ?? findPayload(clip, "Svg");
    if (!payload) {
      setState({ imageUrl: null, loading: false, tooLarge: false });
      return;
    }

    // Guards against a late response for a clip the reader has already moved
    // off, which would otherwise paint the wrong picture into the panel.
    let live = true;
    setState({ imageUrl: null, loading: true, tooLarge: false });

    api
      .getPayload(clip.id, payload.format)
      .then((base64) => {
        if (!live) return;
        const url = base64 ? payloadDataUrl(payload.format, base64) : null;
        setState({ imageUrl: url, loading: false, tooLarge: base64 === null });
      })
      .catch(() => {
        if (live) setState({ imageUrl: null, loading: false, tooLarge: false });
      });

    return () => {
      live = false;
    };
  }, [clip]);

  return state;
}
```

- [ ] **Step 6: Test the hook's three-way selection**

The hook is where this feature is actually wrong or right, and its three branches are invisible in a Playwright run. Create `apps/clipse-app/src/hooks/use-clip-payload.test.ts`:

```ts
import { renderHook, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { useClipPayload } from "./use-clip-payload";
import type { Clip } from "../types/ipc";

const getPayload = vi.fn();
vi.mock("../lib/tauri-client", () => ({
  api: { getPayload: (...args: unknown[]) => getPayload(...args) },
}));

/** A minimal image clip whose single PNG payload is inline or blob-backed. */
function imageClip(body: "Blob" | { Inline: number[] }, size: number): Clip {
  return {
    id: "c1",
    hash: "a".repeat(64),
    kind: "image",
    payloads: [{ format: "Png", digest: "b".repeat(64), size, body }],
    preview: "screenshot",
    source: { device: "d1", device_label: "This machine", app: "Snipping Tool" },
    hlc: { wall_ms: 1, counter: 0, device: "d1" },
    created_at_ms: 1,
    pinned: false,
    deleted: false,
  };
}

describe("useClipPayload", () => {
  beforeEach(() => getPayload.mockReset());

  it("uses inline bytes without asking the daemon", async () => {
    // "hi" as PNG bytes is nonsense, but the hook only cares that the body is
    // inline -- decoding is the browser's problem.
    const { result } = renderHook(() => useClipPayload(imageClip({ Inline: [104, 105] }, 2)));

    await waitFor(() => expect(result.current.imageUrl).toContain("data:image/png;base64,"));
    expect(getPayload).not.toHaveBeenCalled();
  });

  it("asks the daemon for a blob-backed payload", async () => {
    getPayload.mockResolvedValue("aGk=");
    const { result } = renderHook(() => useClipPayload(imageClip("Blob", 500_000)));

    await waitFor(() => expect(result.current.imageUrl).toBe("data:image/png;base64,aGk="));
    expect(getPayload).toHaveBeenCalledWith("c1", "Png");
    expect(result.current.tooLarge).toBe(false);
  });

  it("reports an over-cap payload as too large rather than as an error", async () => {
    getPayload.mockResolvedValue(null);
    const { result } = renderHook(() => useClipPayload(imageClip("Blob", 40_000_000)));

    await waitFor(() => expect(result.current.loading).toBe(false));
    expect(result.current.imageUrl).toBeNull();
    expect(result.current.tooLarge).toBe(true);
  });
});
```

If `@testing-library/react` is not already a dev dependency, add it (`pnpm add -D @testing-library/react`) — the vitest setup at `src/test/setup.ts` already provides a DOM environment.

- [ ] **Step 7: Run the hook tests**

Run: `cd apps/clipse-app && pnpm vitest run src/hooks/use-clip-payload.test.ts`
Expected: PASS, 3 tests.

- [ ] **Step 8: Build the panel**

Create `apps/clipse-app/src/components/peek-panel.tsx`:

```tsx
import { getClipText, humanBytes } from "../lib/clip-content";
import { useClipPayload } from "../hooks/use-clip-payload";
import type { Clip, ClipFormat } from "../types/ipc";
import styles from "./peek-panel.module.css";

function formatLabel(format: ClipFormat): string {
  return typeof format === "string" ? format : format.Other;
}

export interface PeekPanelProps {
  clip: Clip;
  onClose: () => void;
}

/**
 * A clip in full, which the list cannot show.
 *
 * A row is one line, so until now a long clip's content simply had nowhere to
 * be read. The colophon underneath is deliberately the whole record a
 * clipboard manager holds about a clip — including the digest, because a tool
 * that watches everything you copy should be legible about what it kept.
 */
export function PeekPanel({ clip, onClose }: PeekPanelProps) {
  const { imageUrl, loading, tooLarge } = useClipPayload(clip);
  const text = getClipText(clip);
  const biggest = clip.payloads.reduce((a, b) => (a.size >= b.size ? a : b), clip.payloads[0]);

  return (
    <aside className={styles.panel} aria-label="Clip detail">
      <header className={styles.head}>
        <span className={styles.kicker}>{clip.kind}</span>
        <button type="button" className={styles.close} aria-label="Close detail" onClick={onClose}>
          ✕
        </button>
      </header>

      <div className={styles.content}>
        {clip.kind === "image" ? (
          imageUrl ? (
            <img src={imageUrl} alt="" className={styles.image} />
          ) : loading ? (
            <p className={styles.note}>Reading…</p>
          ) : (
            /* Not an error state. Past the 24MB preview cap the size *is* the
             * answer — the clip is intact and pastes normally. */
            <p className={styles.note}>
              {tooLarge ? "Too large to preview" : "No preview"} · {humanBytes(biggest?.size ?? 0)}
            </p>
          )
        ) : (
          <pre className={styles.text}>{text ?? clip.preview}</pre>
        )}
      </div>

      <dl className={styles.meta}>
        <dt>From</dt>
        <dd>{clip.source.app ?? "unknown app"}</dd>
        <dt>Device</dt>
        <dd>{clip.source.device_label}</dd>
        <dt>Copied</dt>
        <dd>{new Date(clip.created_at_ms).toLocaleString()}</dd>
        {clip.payloads.map((p) => (
          <div className={styles.pair} key={`${formatLabel(p.format)}-${p.digest}`}>
            <dt>{formatLabel(p.format)}</dt>
            <dd>{humanBytes(p.size)}</dd>
          </div>
        ))}
        <dt>Hash</dt>
        <dd title={clip.hash}>{clip.hash.slice(0, 12)}</dd>
      </dl>
    </aside>
  );
}
```

Create `apps/clipse-app/src/components/peek-panel.module.css`:

```css
.panel {
  display: flex;
  flex-direction: column;
  width: min(38%, 420px);
  min-width: 16rem;
  border-left: 1px solid var(--color-rule);
  background: var(--color-surface);
  overflow: hidden;
}

/* Below this the list would be squeezed past reading. minWidth is 720, under
 * the switch, so this is a state a person reaches by resizing rather than
 * dead code. */
@media (max-width: 760px) {
  .panel {
    position: absolute;
    inset: 0 0 0 auto;
    width: min(90%, 420px);
    box-shadow: var(--shadow-popup);
    z-index: 5;
  }
}

.head {
  display: flex;
  align-items: baseline;
  justify-content: space-between;
  padding: 1rem 1rem 0.75rem;
  border-bottom: 1px solid var(--color-rule);
}

.kicker {
  font-family: var(--font-mono);
  font-size: var(--type-2xs);
  text-transform: uppercase;
  letter-spacing: var(--tracking-label);
  color: var(--color-ink-muted);
}

.close {
  border: 0;
  background: none;
  color: var(--color-ink-muted);
  font-family: var(--font-mono);
  font-size: var(--type-xs);
  cursor: default;
}

.close:hover { color: var(--color-accent); }

.content {
  flex: 1;
  min-height: 0;
  overflow: auto;
  padding: 1rem;
}

.text {
  margin: 0;
  font-family: var(--font-mono);
  font-size: var(--type-xs);
  line-height: var(--leading-normal);
  color: var(--color-ink-secondary);
  white-space: pre-wrap;
  word-break: break-word;
}

.image {
  display: block;
  max-width: 100%;
  height: auto;
}

.note {
  margin: 0;
  font-family: var(--font-mono);
  font-size: var(--type-xs);
  color: var(--color-ink-muted);
}

/* The colophon: hairline-ruled pairs, not a table. */
.meta {
  display: grid;
  grid-template-columns: 5.5rem 1fr;
  gap: 0.3rem 0.75rem;
  margin: 0;
  padding: 0.85rem 1rem 1rem;
  border-top: 1px solid var(--color-rule);
  font-family: var(--font-mono);
  font-size: var(--type-2xs);
}

.meta dt {
  text-transform: uppercase;
  letter-spacing: var(--tracking-label);
  color: var(--color-ink-muted);
}

.meta dd {
  margin: 0;
  color: var(--color-ink-secondary);
  overflow-wrap: anywhere;
}

.pair {
  display: grid;
  grid-column: 1 / -1;
  grid-template-columns: 5.5rem 1fr;
  gap: 0.75rem;
}
```

- [ ] **Step 9: Wire selection and keys into the history window**

In `history-window.tsx`, add selection state and the keyboard contract:

```tsx
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [peeking, setPeeking] = useState(false);

  const selected = history.clips.find((c) => c.id === selectedId) ?? null;

  // Right opens the panel, Left and Escape close it. Space is deliberately
  // left alone: the list scrolls with it, and stealing that to toggle a panel
  // trades a reflex for a shortcut.
  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (event.key === "ArrowRight" && selected) {
        setPeeking(true);
      } else if (event.key === "ArrowLeft" || event.key === "Escape") {
        setPeeking(false);
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [selected]);
```

Pass `selectedIndex` to `ClipList` from `history.clips.findIndex((c) => c.id === selectedId)`, and change the list's `onActivate` so a click selects and copies:

```tsx
            onActivate={(clip) => {
              setSelectedId(clip.id);
              void handleCopy(clip);
            }}
```

Render the panel as the third column, inside `styles.main`'s parent grid:

```tsx
      {peeking && selected && <PeekPanel clip={selected} onClose={() => setPeeking(false)} />}
```

In `history-window.module.css`, make the window grid accommodate it and give `.main` a positioning context for the overlay mode:

```css
.window {
  display: grid;
  grid-template-columns: auto 1fr auto;
}

.main {
  position: relative;
}
```

- [ ] **Step 10: Teach the fixture the new command**

In `apps/clipse-app/e2e/fixtures/tauri-stub.ts`, add a case to `invoke`'s switch, before `default`:

```ts
      case "get_payload": {
        const clip = clips.find((c) => c.id === args.id);
        if (!clip) return null;
        const payload = clip.payloads.find((p) => p.format === args.format);
        if (!payload || payload.body === "Blob") return null;
        let binary = "";
        for (const byte of payload.body.Inline) binary += String.fromCharCode(byte);
        return btoa(binary);
      }
```

- [ ] **Step 11: Write the Playwright test**

Add to `apps/clipse-app/e2e/history.spec.ts`:

```ts
test("the detail panel opens on Right and closes on Escape", async ({ page }) => {
  const panel = page.getByRole("complementary", { name: "Clip detail" });
  await expect(panel).toBeHidden();

  // Selecting a row is what gives Right something to open.
  await page.getByRole("option").first().click();
  await page.keyboard.press("ArrowRight");
  await expect(panel).toBeVisible();

  // The row is one line; the panel is where the whole clip can be read.
  await expect(panel).toContainText("Device");
  await expect(panel).toContainText("Hash");

  await page.keyboard.press("Escape");
  await expect(panel).toBeHidden();
});
```

- [ ] **Step 12: Verify**

Run: `cd apps/clipse-app && pnpm vitest run && pnpm tsc --noEmit && pnpm exec playwright test`
Expected: green.

- [ ] **Step 13: Commit**

```bash
git add apps/clipse-app
git commit --no-gpg-sign -m "Let a clip be read in full

A row is one line, so a long clip's content had nowhere to be read and its
provenance -- which app, which device, which digest -- was nowhere at all.
Right opens the panel, Left and Escape close it. Space is left alone: the
list scrolls with it, and taking that for a panel toggle trades a reflex
for a shortcut.

Screenshots work because the panel goes through get_payload rather than
hoping for an inline body. Past the 24MB cap the size is shown, which is
not an error state -- the clip is intact and pastes normally.

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>"
```

---

### Task 8: Settings as an editorial grid

**Files:**
- Modify: `apps/clipse-app/src/pages/settings-view.tsx`, `settings-view.module.css`
- Modify: `apps/clipse-app/src/pages/history-window.tsx` (hold the spine across both views)

**Interfaces:**
- Consumes: `<Spine />` (Task 6), `<AsciiLogo />` (Task 4).
- Produces: `<SettingsView status={DaemonStatus | null} />` — the `onBack` prop is gone, because the spine owns navigation now. Everything else about the component's internals (`useSettings`, the `draft`/`dirty` state, the `Row` helper) is unchanged.

- [ ] **Step 1: Write the failing test**

Add to `apps/clipse-app/e2e/history.spec.ts`:

```ts
test("settings keeps the spine and returns on Escape", async ({ page }) => {
  await page.getByRole("button", { name: "Settings" }).click();
  await expect(page.getByRole("heading", { name: "Capture" })).toBeVisible();

  // Not a separate screen: the rail it was opened from is still there, and
  // the control that opened it reads as pressed.
  await expect(page.getByRole("button", { name: "Settings" })).toHaveAttribute(
    "aria-pressed",
    "true",
  );
  await expect(page.getByRole("button", { name: "← Back" })).toHaveCount(0);

  await page.keyboard.press("Escape");
  await expect(page.getByRole("listbox", { name: "Clipboard history" })).toBeVisible();
});
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cd apps/clipse-app && pnpm exec playwright test history.spec.ts -g "settings keeps the spine"`
Expected: FAIL — the `← Back` button still exists, there are no section headings, and settings replaces the whole window including the spine.

- [ ] **Step 3: Hold the spine across both views**

In `history-window.tsx`, stop returning early for the settings view. Replace the `if (view === "settings") { return <SettingsView … /> }` block by rendering the spine once and switching only the main column:

```tsx
      <Spine
        /* …props as in Task 6… */
        settingsActive={view === "settings"}
        onOpenSettings={() => setView(view === "settings" ? "history" : "settings")}
      />

      <div className={styles.main}>
        {view === "settings" ? (
          <SettingsView status={status} />
        ) : (
          <>{/* the search row, banners and list from Task 6 */}</>
        )}
      </div>
```

Extend the keydown handler from Task 7 so Escape leaves settings before it closes a panel:

```tsx
      } else if (event.key === "Escape") {
        if (view === "settings") setView("history");
        else setPeeking(false);
      }
```

Add `view` to that effect's dependency array.

- [ ] **Step 4: Rebuild settings as numbered sections**

In `settings-view.tsx`: delete the `onBack` prop and the whole `<header>` with its back button. Wrap each `<section>` with a numbered kicker matching the onboarding's index/kicker pair, and keep the existing `Row` children unchanged:

```tsx
function Section({ index, title, children }: { index: string; title: string; children: React.ReactNode }) {
  return (
    <section className={styles.section}>
      <div className={styles.sectionHead}>
        <span className={styles.index} data-numeric>
          {index}
        </span>
        <h2 className={styles.sectionTitle}>{title}</h2>
        <span className={styles.sectionRule} aria-hidden="true" />
      </div>
      {children}
    </section>
  );
}
```

Wrap the four existing groups as `01 Capture` (hotkey, apply incoming, detect secrets, start at login), `02 This device` (device label, announce, quota), `03 Never captured` (blocked apps), `04 Devices` (pairing). Replace the colophon's `<p className={styles.colophonTitle}>Clipse</p>` with `<AsciiLogo variant="wordmark" cell={4.5} />`.

- [ ] **Step 5: Re-grid the rows and pin the save bar**

In `settings-view.module.css`:

```css
/* Asymmetric on purpose: the explanation gets a measure it can be read at,
 * and the control sits at the right edge where the eye returns. */
.row {
  display: grid;
  grid-template-columns: minmax(0, 22rem) 1fr;
  align-items: start;
  gap: 2rem;
  padding: 1rem 0;
  border-bottom: 1px solid var(--color-rule);
}

.row.stacked {
  grid-template-columns: 1fr;
  gap: 0.75rem;
}

.rowControl {
  justify-self: end;
}

.sectionHead {
  display: grid;
  grid-template-columns: auto auto 1fr;
  align-items: center;
  gap: 0.75rem;
  padding-top: 1.75rem;
  padding-bottom: 0.5rem;
}

.index {
  font-family: var(--font-mono);
  font-size: var(--type-2xs);
  color: var(--color-accent-ink);
  font-variant-numeric: tabular-nums;
}

.sectionTitle {
  margin: 0;
  font-family: var(--font-display);
  font-weight: var(--weight-heading);
  font-size: var(--type-md);
  letter-spacing: var(--tracking-tight);
  color: var(--color-ink);
}

/* A rule that runs to the edge, so the heading sits in the margin rather
 * than on top of a box. */
.sectionRule {
  height: 1px;
  background: var(--color-rule);
}

/* Only present when there is something to save. A permanently visible save
 * bar is a row of chrome that spends most of its life saying nothing. */
.saveBar {
  position: sticky;
  bottom: 0;
  display: flex;
  align-items: center;
  justify-content: flex-end;
  gap: 1rem;
  padding: 0.85rem 0;
  border-top: 1px solid var(--color-rule);
  background: var(--color-canvas);
}
```

In `settings-view.tsx`, render the save bar only when it has something to report:

```tsx
            {(dirty || saving || savedFlash || errorMessage) && (
              <div className={styles.saveBar}>
                {/* …existing contents… */}
              </div>
            )}
```

The accent on `.index` is the section numeral, which is never on screen at the same time as a hovered close control — but check this against the one-red rule while driving it, and if the numerals read as decoration rather than structure, drop them to `--color-ink-muted`.

- [ ] **Step 6: Verify**

Run: `cd apps/clipse-app && pnpm vitest run && pnpm tsc --noEmit && pnpm exec playwright test`
Expected: green, including the new settings test. Existing settings tests that clicked `← Back` must press Escape instead.

- [ ] **Step 7: Commit**

```bash
git add apps/clipse-app
git commit --no-gpg-sign -m "Set settings on the same grid as everything else

It was a stack of label-and-control rows behind a back button, which made
it a separate screen for something that is a second view of one window. The
spine stays mounted, the control that opened it reads as pressed, and
Escape returns -- so there is no back button to draw.

Sections are numbered the way the onboarding numbers its steps, rows are
asymmetric so an explanation gets a readable measure, and the save bar only
appears when there is something to save.

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>"
```

---

### Task 9: Record the decisions and correct the project map

**Files:**
- Modify: `docs/decisions.md`
- Modify: `docs/manual-verification.md`
- Modify: `apps/clipse-app/src-tauri/tauri.conf.json` (only if Step 10 of Task 5 found a blocking snap problem)

- [ ] **Step 1: Write the decision entries**

Append to `docs/decisions.md`, in the file's existing style:

- **A frameless main window, with our own controls on every platform.** Why: the native Windows title bar sat above the editorial masthead as a second header. Why not a drawn title bar: the same problem in our own colours. Why not per-platform: two masthead variants to maintain for one window. What it costs: the Win32 resize border and snap affordance, replaced by hand — see `lib/window-frame.ts` and the log entry in `manual-verification.md`.
- **`IPC_VERSION` 1 → 2, once, for two additions.** Why a single bump: the `Hello` handshake refuses a mismatch outright (`clipse-ipc/src/client.rs:48`), so both sides are guaranteed to agree and neither `secrets_refused` nor `GetPayload` needs a fallback path. Why the network `PROTOCOL_VERSION` is untouched: no `ClipFormat::label()` string and no part of the content-hash layout changed.
- **A 24MB preview cap, and `serde_bytes` on the response.** Why a cap at all: a 400MB file copy is a perfectly good clip and a terrible thing to pull into a webview. Why 24MB: clears a 4K screenshot with 8MB of headroom under `MAX_FRAME_BYTES`. Why `serde_bytes`: without it a `Vec<u8>` encodes as a MessagePack integer array at ~1.5x, and the cap would be an artefact of the encoding rather than a decision.
- **The suppression count is never persisted.** Why: a durable tally means writing a record about the thing Clipse promised not to write down. "Since Clipse started" costs nothing and is true.

- [ ] **Step 2: Finish the manual-verification entry**

Ensure the dated entry started in Task 5 Step 10 records all seven checks with their real outcomes, including any that failed. A log that only lists successes is not a log.

- [ ] **Step 3: Commit**

```bash
git add docs/
git commit --no-gpg-sign -m "Record the frame, IPC and preview-cap decisions

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>"
```

- [ ] **Step 4: Correct the project map**

The hub map's stated type stack (Instrument Serif + IBM Plex Mono) contradicts `styles/tokens.css` (Bricolage Grotesque + DM Mono). Call `project_update` on `clipse` with the corrected stack, the new `current_focus`, and `next_steps` carrying forward both this plan's deferred list and the pre-existing items the map already holds — the first cross-device sync test, the F3 notch check, and the F1 password-manager check. A stale map actively misleads the next agent.

---

## Deferred (from the spec, not lost)

Text transforms on a clip · a `?` cheat-sheet and a "show the introduction again" control · sequential paste · named snippets · filters by device, date and source app · a Windows jump list.
