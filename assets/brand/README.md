# Clipse brand assets

All artwork in this directory is hand-written, resolution-independent SVG. No file depends on an external font, bitmap, or background colour.

## Core assets

- `clipse-mark.svg` — full-colour eclipse mark on transparency; use for product branding from 16px through 512px.
- `clipse-mark-mono.svg` — bold `currentColor` mark for OS-tinted tray and menu-bar placements.
- `clipse-logo.svg` — full mark plus path-outlined wordmark for dark surfaces.
- `clipse-logo-light.svg` — deeper corona and dark path-outlined wordmark for light surfaces.
- `app-icon.svg` — opaque 1024px installer icon with a night rounded tile and restrained corona glow.
- `tokens.css` — source palette, semantic light/dark colours, spring easing, and type scale.

## Interface icons

Every icon inherits `currentColor`, has a `0 0 24 24` viewBox, and omits fixed dimensions.

- `icons/pin.svg` — pin an item.
- `icons/pin-filled.svg` — filled active state for a pinned item.
- `icons/search.svg` — search clipboard history.
- `icons/text.svg` — identify plain-text content.
- `icons/image.svg` — identify image content.
- `icons/file.svg` — identify file content.
- `icons/link.svg` — identify or open linked content.
- `icons/devices.svg` — show paired or syncing devices.
- `icons/pause.svg` — pause clipboard capture or sync.
- `icons/play.svg` — resume clipboard capture or sync.
- `icons/settings.svg` — open application settings.
- `icons/trash.svg` — delete an item or clear history.
- `icons/copy.svg` — copy an item back to the clipboard.
- `icons/check.svg` — confirm success or selection.
- `icons/shield.svg` — indicate privacy protection or a suppressed secret.
- `icons/wifi-off.svg` — indicate offline state.
- `icons/laptop.svg` — identify a laptop device.
- `icons/desktop.svg` — identify a desktop device.
- `icons/close.svg` — dismiss a surface.
- `icons/chevron-down.svg` — reveal a menu or collapsed section.

## Editing rules

1. The crescent is a real mask cut-out. Never replace it with a background-coloured disc or other overlay.
2. Keep the 135-degree corona within the supplied amber ramp, and keep UI icons at a 1.75-unit stroke with round caps and joins.
3. Do not introduce text elements, external fonts, raster images, or fixed icon dimensions. Preserve the small-size silhouette before adding detail.
