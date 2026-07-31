# Installer artwork: the identity survives the wizard

**Date:** 2026-07-31
**Status:** approved, not yet implemented

## Why

The application has one picture of itself: an eclipse computed per frame from a
character grid, drawn hot red on a near-black room. The onboarding is built on
it, the mark is cut from it, the app icon was regenerated from it.

The installers know nothing about any of that.
`apps/clipse-app/src-tauri/tauri.conf.json` carries exactly two Windows options
— `nsis.installMode` and `wix.language` — and nothing at all under
`macOS.dmg`. So the first thing anyone ever sees of Clipse is NSIS's stock
grey-blue sidebar, MSI's stock banner, and a bare Finder window with two icons
in it. Three pieces of somebody else's default art, on the one screen where a
person decides whether this is a serious program.

The gap is not decorative. Someone who installs this is being asked to trust a
program with every password they copy. Arriving through a wizard that looks
like a 2004 shareware installer is an argument against that trust, made before
the product has spoken a word.

## Goals

- Every installer surface on Windows and macOS is drawn from the same character
  grid the running application draws from — not from a picture that resembles
  it.
- The artwork is *generated*, deterministic, and regenerable from a command.
- No new dependency, in either the Node or the Cargo graph.
- Installer text stays legible. An unreadable welcome dialog is worse than a
  stock one.

## Non-goals

- **Custom NSIS or WiX page templates.** Both bundlers accept a `template`
  override. Taking it means owning the whole installer script against Tauri's
  upgrades, for pages that are mostly OS-drawn text. Bitmaps get most of the
  effect for a fraction of the surface.
- **Prose text inside the artwork.** Rasterising real type needs a font
  rasteriser, which needs a dependency. NSIS, MSI and Finder all draw their own
  text over or beside these images anyway.
- **The in-app onboarding.** It already speaks this language and is out of
  scope here.
- **Signing and notarisation.** Unchanged, and still unconfigured for the
  reasons in `docs/packaging.md`.

## The surfaces, and what each one actually allows

Dimensions and formats below are from the bundled Tauri config schema
(`@tauri-apps/cli@2.11.4/config.schema.json`), not from memory.

| Asset | Size | Where it appears | Background constraint |
| --- | --- | --- | --- |
| `nsis-sidebar.bmp` | 164×314 | NSIS Welcome and Finish pages, full left panel | May be fully dark — page text sits beside it, not on it |
| `nsis-header.bmp` | 150×57 | Header of every other NSIS page, top right | A dark plate on the installer's white header strip |
| `wix-banner.bmp` | 493×58 | MSI top strip on all but the first page | **Left side must be light** — MSI draws black text there |
| `wix-dialog.bmp` | 493×312 | Entire background of the MSI Welcome and Exit dialogs | **Right ~330px must be light** — black title and body text are drawn over it |
| `dmg-background.png` | 660×420 | The Finder window the DMG opens | Free |

The two WiX rows are the load-bearing ones. `dialogImagePath` is not a panel,
it is the whole dialog background, and MSI paints its heading and body text
straight onto it in a fixed dark colour. A uniformly black image there produces
an installer screen nobody can read. The stock WiX bitmap solves this by
putting artwork in a left band and leaving the rest white, and that division is
not a stylistic choice we get to reject.

`uninstallerHeaderImage` reuses the installer header. An uninstaller that
suddenly looks like a different program is its own small alarm.

## Composition

Two elements, no others: the **eclipse field** and the **`CLIPSE_WORDMARK`
grid**. The absence of prose is a constraint honoured as a rule, not a
shortfall — the surfaces that need words already have them.

The wordmark is rendered as solid cells rather than as `#` characters. At 164px
across, 35 cells leave roughly four pixels each; a `#` glyph drawn into four
pixels is noise. The grid is still literally the same grid.

**NSIS sidebar (164×314).** Vertical. Eclipse near totality in the upper two
thirds, wordmark below it, a hairline rule at the foot. Totality is chosen
deliberately: the disc is darkest on the page where the installer is asking to
be trusted.

**NSIS header (150×57).** Too small for the wordmark. A crop of the corona
only — a dark plate with red characters, sitting on the white header strip.

**WiX dialog (493×312).** Two zones. Left 165px is the NSIS sidebar's
composition, narrowed. The remaining width is flat `--lit-50` (`#F4F8F6`) so
MSI's text lands on a light field. A one-pixel `--signal-500` rule marks the
seam.

**WiX banner (493×58).** Inverted for the same reason: light field on the left
where MSI writes, eclipse crop and wordmark pushed to the right edge.

**DMG background (660×420).** No arrow. The field thins out at the two icon
positions, leaving the app icon and the Applications folder each sitting in a
clearing, and between them the ramp brightens left to right — direction carried
by the characters themselves rather than by a drawn arrow over them. Finder
supplies both icon labels.

If the gesture does not read on a real Mac, the fallback is chevrons built from
ramp characters in the same track. That decision needs the physical machine and
is recorded as unverified below rather than guessed at now.

## Palette

Canvas `--void-950` `#020806`. Corona ramp interpolated across `--signal-700`
`#9E141B` → `--signal-500` `#FB3640` → `--signal-300` `#FF8A8F`, driven by the
ramp index. The solid disc is `--signal-500`. The wordmark is `--lit-100`
`#E3EBE7`.

Read the palette from `apps/clipse-app/src/styles/tokens.css`. The amber values
in `assets/brand/tokens.css` are the superseded palette and must not be used
here; the running eclipse is red (`components/eclipse-canvas.module.css`).

The wordmark being neutral rather than red is deliberate and follows the lesson
already paid for twice in this project: the room reads as black only when
something genuinely neutral is held next to it.

## Code

**`apps/clipse-app/src/lib/ascii-raster.ts`** — pure, no I/O. Hand-built 5×7
pixel glyphs for the twelve ramp characters `.,-~:;=!*#$@`, and
`rasterize(lines, options) → { width, height, rgb }`. It sits beside
`ascii-logotype.ts` and inherits its discipline: every glyph row is the same
length, and a test enforces it, because a glyph one pixel short leans a
character by a fraction of a cell — visible, but not obviously wrong.

It is imported by no UI code, so it costs the bundle nothing.

**`apps/clipse-app/scripts/render-installer-art.mts`** — imports `render()`
from `lib/eclipse-ascii.ts` and `CLIPSE_WORDMARK` from `lib/ascii-logotype.ts`,
composes the five images, and writes them. Exposed as `pnpm art`.

**Writers.** BMP is written by hand: 24-bit, uncompressed, bottom-up rows
padded to four bytes. That is the form NSIS and WiX both accept without
argument; RLE and 32-bit-with-alpha variants are where installer bitmaps
usually fail. PNG goes through `node:zlib`, which ships with Node.

No package is added to either graph.

**Output lives in `assets/installer/`** and is committed. Regenerating is a
command, not a build step: CI should not need a rasteriser, and a change to the
artwork should be visible in a diff rather than only in a built artefact. This
follows how `assets/launch/` is already handled.

```
assets/installer/
  README.md
  windows/nsis-sidebar.bmp
  windows/nsis-header.bmp
  windows/wix-banner.bmp
  windows/wix-dialog.bmp
  macos/dmg-background.png
```

**`tauri.conf.json`** gains `nsis.headerImage`, `nsis.sidebarImage`,
`nsis.uninstallerHeaderImage`, `wix.bannerPath`, `wix.dialogImagePath`, and a
`macOS.dmg` block with `background`, `windowSize`, `appPosition` and
`applicationFolderPosition`. Paths there resolve relative to the `src-tauri`
directory.

## Testing

- `ascii-raster.test.ts`: every glyph is rectangular and 5×7; the ramp is
  covered without gaps; `rasterize` returns exactly `width × height × 3` bytes;
  a known small input produces a known pixel.
- BMP writer: header fields, and the file's declared dimensions equal the ones
  the schema requires for each asset.
- PNG writer: output round-trips through `zlib.inflateSync` and carries a valid
  signature and IHDR.
- A test asserts the five committed assets exist at their exact required
  dimensions, so a regeneration that silently changes a size fails CI rather
  than the installer.

## Verification, and its limits

**Windows can be verified here and will be.** `pnpm tauri build --bundles
nsis,msi` on the development machine, then run the produced setup and step
through Welcome → License → Install → Finish, and the MSI equivalent, looking
at each of the four bitmaps in place.

**macOS cannot be verified from here.** Whether the clearings in the DMG
background line up with `appPosition` and `applicationFolderPosition` is only
answerable in front of a Mac. It goes into `docs/manual-verification.md` as an
open item and will not be described as working until someone has looked at it.

**Retina.** Tauri accepts `png`/`jpg`/`gif` for the DMG background and not a
multi-representation `.tiff`, so there is no way to supply an `@2x` layer. The
background will be soft on a Retina display. This is a limit of the bundler,
not a choice, and it is written down here so nobody re-litigates it later.

**One NSIS assumption is worth naming.** The sidebar is shown on the MUI
Welcome and Finish pages; the plan assumes Tauri's generated script presents
those pages under `installMode: currentUser`. The Windows build above is what
settles it. If those pages turn out not to be shown, the sidebar work is dead
weight and the header bitmap carries the whole Windows result.
