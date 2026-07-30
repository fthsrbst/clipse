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
signing when they are absent:

| Secret | Used for |
| --- | --- |
| `WINDOWS_CERTIFICATE`, `WINDOWS_CERTIFICATE_PASSWORD` | Authenticode |
| `APPLE_CERTIFICATE`, `APPLE_CERTIFICATE_PASSWORD`, `APPLE_SIGNING_IDENTITY` | codesign |
| `APPLE_ID`, `APPLE_PASSWORD`, `APPLE_TEAM_ID` | notarytool |
| `TAURI_SIGNING_PRIVATE_KEY`, `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` | update signing |

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
