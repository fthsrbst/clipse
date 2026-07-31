# Installer artwork

Five images, generated. Regenerate them with:

```bash
cd apps/clipse-app && pnpm art
```

The generator is `apps/clipse-app/scripts/render-installer-art.mts`; the parts
with judgement in them — the glyphs, the ramp, the two encoders — are in
`apps/clipse-app/src/lib/{ascii-raster,image-encode}.ts`, under test.

Nothing here is drawn by hand or exported from a design tool. The eclipse comes
out of `lib/eclipse-ascii.ts`, the same function the running application calls
every frame, and the wordmark out of `lib/ascii-logotype.ts`. That is the point:
the installer and the product are the same drawing, not two drawings that
resemble each other.

## The files

| File | Size | Used by |
| --- | --- | --- |
| `windows/nsis-sidebar.bmp` | 164×314 | `nsis.sidebarImage` — Welcome and Finish pages |
| `windows/nsis-header.bmp` | 150×57 | `nsis.headerImage` and `nsis.uninstallerHeaderImage` |
| `windows/wix-banner.bmp` | 493×58 | `wix.bannerPath` — MSI top strip |
| `windows/wix-dialog.bmp` | 493×312 | `wix.dialogImagePath` — MSI Welcome and Exit |
| `macos/dmg-background.png` | 660×420 | `macOS.dmg.background` |

The sizes are NSIS's and WiX's, not ours. A bitmap of the wrong dimensions is
not rejected — it is stretched, cropped, or silently ignored.
`src/test/installer-assets.test.ts` asserts every one of them, and asserts that
`tauri.conf.json` still points here.

## Rules

1. **The Windows bitmaps must stay 24-bit, uncompressed, bottom-up.** That is
   the one shape every version of NSIS and WiX accepts. RLE and
   32-bit-with-alpha are where installer bitmaps quietly fail to appear at all.
2. **`wix-dialog.bmp` keeps its light right-hand zone, and `wix-banner.bmp`
   stays light on the left.** MSI paints its heading and body text straight onto
   those bitmaps in a fixed dark colour. A uniformly dark one is an installer
   screen nobody can read. WixUI's own text runs to roughly 406 of these
   pixels on the banner and starts at about 180 on the dialog.
3. **The DMG background's clearings must agree with `appPosition` and
   `applicationFolderPosition`.** The holes in the corona are where the icons
   go. Move one without the other and the icons land on top of the drawing.
4. **The palette is `apps/clipse-app/src/styles/tokens.css`.** The amber values
   in `assets/brand/tokens.css` are the superseded palette; the eclipse is red.

## Not verified

The DMG has never been opened. Whether the icons sit in their clearings is
answerable only in front of a Mac — see `docs/manual-verification.md`.
