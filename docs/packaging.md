# Packaging

`.github/workflows/release.yml` builds installers for all three platforms on a
`v*` tag. It produces artefacts whether or not signing credentials exist — a
fork gets unsigned installers and a warning, not a failed build twenty minutes
in.

## What is configured

`apps/clipse-app/src-tauri/tauri.conf.json` carries the bundle targets (MSI and
NSIS on Windows, DMG and .app on macOS, AppImage and .deb on Linux), the
`.deb` runtime dependencies the clipboard backends need, and a content security
policy restricting the webview to its own bundled assets. That CSP is a privacy
control, not a formality: it is what stops a compromised dependency from
posting a clipboard history to a remote host.

The daemon is built separately and shipped alongside the app. Closing the
window must not stop syncing, so the installer needs both binaries.

## The installer artwork

`assets/installer/` holds five generated images, wired into the bundle config:
the NSIS sidebar and header, the WiX banner and dialog, and the DMG background.
They are drawn by `pnpm art` in `apps/clipse-app` from the same eclipse
renderer the running application uses, and committed — a release does not need
a rasteriser, and a change to the artwork shows up in a diff.

Two constraints in there are not stylistic and should not be "fixed" by
somebody making the installer look more consistent:

- **The MSI images have a light zone.** MSI paints its heading and body text
  onto `wix-dialog.bmp` and its page title onto `wix-banner.bmp`, in a fixed
  dark colour. Make them uniformly dark and the installer becomes unreadable.
- **The DMG background's clearings encode `appPosition` and
  `applicationFolderPosition`.** The holes in the corona are where Finder puts
  the icons. The two have to move together.

`assets/installer/README.md` has the rest, and
`docs/manual-verification.md` has the list of things that have to be looked at
rather than asserted.

## What each platform gets

| Platform | Assets |
| --- | --- |
| Windows | `.msi`, `-setup.exe` (NSIS), `-portable.exe` |
| macOS | `.dmg` |
| Linux | `.AppImage`, `.deb` |

**The portable executable** is the same binary the installer would place, taken
before it is wrapped. The app runs the daemon inside its own process, so one
file really is the whole product. Two things it is not: it still needs the
WebView2 runtime, which ships with Windows 10 22H2 and 11 but not with older
installs; and "portable" means "nothing to install", not "leaves no trace" —
history goes to the same per-user directory an installed copy uses.

## Tagging publishes

A `v*` tag runs `bundle` on all three runners and then `publish`, which creates
the release if it does not exist and attaches every installer to it. Before
this existed, a tag produced artefacts you had to go and find in the Actions
tab, and `v0.1.0`'s assets were uploaded by hand.

`publish` matches assets by shape rather than by extension. The bare
`clipsed.exe` is kept as a build artefact for debugging, and matching every
`.exe` would have put it on the release page looking like something to
download.

## What is *not* configured, and why

**Signing.** Windows needs an Authenticode certificate; macOS needs a Developer
ID plus an app-specific password for notarisation. Both belong to the project
owner and cannot be checked in. The workflow reads them from secrets and skips
signing when they are absent — on macOS with one correction described below:

| Secret | Used for |
| --- | --- |
| `WINDOWS_CERTIFICATE`, `WINDOWS_CERTIFICATE_PASSWORD` | Authenticode |
| `APPLE_CERTIFICATE`, `APPLE_CERTIFICATE_PASSWORD`, `APPLE_SIGNING_IDENTITY` | codesign |
| `APPLE_ID`, `APPLE_PASSWORD`, `APPLE_TEAM_ID` | notarytool |
| `TAURI_SIGNING_PRIVATE_KEY`, `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` | update signing |

### macOS without a Developer ID

"Skips signing" was too literal, and it produced a broken product. With no
identity Tauri left the `.app` carrying only the ad-hoc signature the linker
puts on the executable, and no seal over the bundle: `codesign --verify` failed
with *"code has no resources but signature indicates they must be present"*,
and Gatekeeper turned that into **"Clipse is damaged and can't be opened. You
should move it to the Trash."** — the one refusal that gives a user nothing to
try. Every macOS asset up to and including `v0.2.2` had this. Confirmed by
inspecting the published dmg, not by reasoning about it.

The workflow now sets `APPLE_SIGNING_IDENTITY=-` when no real identity is
present, so the bundle is signed ad-hoc and verifies. What that does and does
not buy:

- `codesign --verify --deep --strict` passes, and the bundle keeps its
  identifier (`dev.clipse.app`) and hardened runtime.
- It is still **not notarised**, so `spctl` still rejects it and a downloaded
  copy is stopped once, with the "Apple could not verify…" dialog that does
  offer *Open Anyway*. The README tells users about that and about
  `xattr -dr com.apple.quarantine`.
- Only a real Developer ID plus notarisation makes the first launch silent.
  The secrets above are already wired for it; nothing else has to change.

### The dmg's window was never actually configured

Tauri passes create-dmg `--skip-jenkins` whenever `CI` is set, and that flag
skips the AppleScript that writes the volume's `.DS_Store`. The window size,
the icon positions and the background all live in that file. So the released
dmgs shipped `.background/dmg-background.png` and no instruction to display it:
they opened as a bare Finder window. Setting `TAURI_BUNDLER_DMG_IGNORE_CI=true`
asks for the styling; the workflow falls back to an unstyled dmg rather than
failing a release if Finder cannot be driven on the runner.

Two things follow for anyone building this by hand:

- **Unmount any volume already called `Clipse` first.** create-dmg mounts the
  staging volume by name, and an existing one makes the AppleScript fail with
  `-1728` and the whole build fail with the useless `error running
  bundle_dmg.sh`.
- **`windowSize.height` is the artwork plus the title bar.** Finder's bounds
  include the 28pt title bar; the background is drawn in the content area under
  it. See `scripts/render-installer-art.mts`.

**The auto-updater is switched off.** Its config exists but `active` is `false`
and `pubkey` is empty, deliberately. The updater verifies a signature over
every release it downloads; enabling it without a real key would ship an
auto-update channel that trusts whatever it is handed — a worse outcome than
having no updater at all. To turn it on:

```bash
pnpm tauri signer generate -w ~/.tauri/clipse.key
```

Put the public half in `plugins.updater.pubkey`, set `active` to `true`, and
add the private half to `TAURI_SIGNING_PRIVATE_KEY`.

## Unverified

These installers have never been built. The workflow is written against
Tauri's documented inputs and the same system dependencies CI already uses to
compile the app, but no artefact has been produced, installed, or launched on a
clean machine. Until someone tags a release and installs the result, treat this
as a plan rather than a fact — and see the F4 section of
`docs/manual-verification.md` for what to check when they do.
