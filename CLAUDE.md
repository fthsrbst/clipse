# Clipse — working notes for agents

Read this before changing anything. The project map in the `hub` MCP server
(`project_get("clipse")`) carries current focus and next steps.

## Ground rules

1. **`clipse-core` is the contract.** Every other crate depends on it; it
   depends on none of them and contains no I/O, async or platform code. Change
   it only when the domain genuinely changes, and update every dependant in the
   same commit.
2. **Privacy rules are code, not settings.** A clip flagged sensitive must never
   reach the store or the network. If you touch the capture path, keep the
   sensitive-content tests green.
3. **The daemon owns the truth.** The Tauri app is a view over `clipsed` via
   `clipse-ipc`. Never let the UI talk to the store or the network directly.
4. **Never change `ClipFormat::label()` strings or the content-hash layout**
   without bumping `PROTOCOL_VERSION` — those strings are baked into every
   stored row and every wire message.

## Commands

```bash
cargo test --workspace          # unit + integration
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all
cargo run -p clipsed -- --data-dir ./.clipse-dev/a   # run a daemon
```

## Conventions

- Rust 2024 edition, `rustfmt.toml` at the root, clippy is deny-warnings in CI.
- Comments explain constraints the code cannot show (why a clamp exists, why an
  order is canonical) — not what the next line does.
- Errors: `thiserror` enums per crate, `anyhow` only in binaries.
- Tests live next to the code in `mod tests`; cross-crate scenarios go in
  `tests/`.
- Frontend: TypeScript strict, kebab-case files, PascalCase types.

## Things that will bite you

- **SQLCipher needs OpenSSL build tooling.** The `encryption` feature of
  `clipse-store` is off by default so a plain `cargo test` works on a machine
  without Perl/NASM; CI and release builds turn it on. See
  `docs/decisions.md`.
- **GNOME Wayland has no `wlr-data-control`.** There is no background clipboard
  monitoring there; Clipse falls back to manual push and says so in the UI.
- **mDNS does not work on a tailnet** (no multicast). Tailnet peers are resolved
  from `tailscale status --json` against addresses recorded at pairing time.
