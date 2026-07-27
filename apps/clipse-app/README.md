# clipse-app

The Clipse desktop interface: tray icon, history window, and the global-hotkey
popup. It is a *view* over the `clipsed` daemon — it talks to it over
`clipse-ipc` and never touches the store or the network itself.

## Running against the real daemon

Two processes. Point both at the same scratch directory so nothing here
touches a real clipboard history.

```bash
cargo run -p clipsed -- --data-dir ./.clipse-dev/a --log debug
```

```bash
cd apps/clipse-app && CLIPSE_DATA_DIR=../../.clipse-dev/a pnpm tauri dev
```

On PowerShell, set the variable first:

```bash
$env:CLIPSE_DATA_DIR = "..\..\.clipse-dev\a"; pnpm tauri dev
```

Without `CLIPSE_DATA_DIR` the app uses the platform data directory, which is
what a real installation does.

## Running against the mock daemon

The mock speaks the real protocol over the real transport and serves a handful
of fabricated clips (text, a long text, an HTML+text pair, an image). It is
the fastest way to work on the frontend, and it needs no clipboard access.

```bash
cargo run -p clipse-app --example mock-daemon -- ./.clipse-dev/mock
```

```bash
cd apps/clipse-app && CLIPSE_DATA_DIR=../../.clipse-dev/mock pnpm tauri dev
```

## Frontend only

`pnpm dev` serves the UI on its own. Tauri commands are unavailable, so the
app renders its "daemon not running" state — which is itself worth looking at.

## Checks

```bash
pnpm exec tsc --noEmit
```

```bash
pnpm run test
```

```bash
pnpm run build
```

```bash
pnpm run test:e2e
```

## Layout

| Path | What lives there |
| --- | --- |
| `src/pages/` | The three surfaces: history window, popup, settings |
| `src/components/` | Presentational pieces, each with its own CSS module |
| `src/hooks/` | Daemon connection, history, settings, list virtualisation |
| `src/lib/` | Pure logic: fuzzy match, popup keyboard reducer, relative time |
| `src/styles/tokens.css` | Design tokens, copied from `assets/brand/` |
| `src-tauri/src/` | Connection, commands, tray, hotkey, popup positioning |
| `e2e/` | Playwright specs with the Tauri bridge stubbed |

Which surface renders is decided by the Tauri window label — see
`src/lib/window-labels.ts` and `src-tauri/src/popup.rs`.

## Rules

- Style against the semantic tokens (`--color-*`, `--shadow-*`, `--radius-*`,
  `--duration-*`). Never reference `--night-*` or `--amber-*` from a component.
- No network requests at runtime, ever. Inter is vendored in `public/fonts/`
  and icons are inlined from `assets/brand/icons/`. No CDN, no analytics.
- The popup must stay fully operable from the keyboard alone.
