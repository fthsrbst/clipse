# Launch assets

## `clipse-hero-1920x1080.png`

The still. The eclipse is a real frame from the product's own renderer
(`apps/clipse-app/src/lib/eclipse-ascii.ts` at phase 0.5, totality) rendered at
78×30 characters, so the poster and the running application are drawing the same
picture rather than two pictures that resemble each other.

Regenerate it with `totality-78x30.txt` as input; the renderer is deterministic
apart from the corona's time term.

**One honest caveat.** The still is rasterised through `System.Drawing`, which
cannot load `.woff2`, so it is set in Georgia and Consolas rather than the
product's Instrument Serif and IBM Plex Mono. Georgia is close in spirit — an
editorial serif with real contrast — but it is not the brand face. If this is
ever used somewhere it matters, reset the type in the real faces.

## Video prompt

Feed the still to an image-to-video model with the prompt below. It is written
for a **6–8 second** clip that loops cleanly, and it deliberately asks for very
little: the still is already the composition, and the job of the motion is to
make it breathe, not to stage a new scene.

> A total solar eclipse rendered entirely in glowing amber ASCII characters on a
> deep warm-black background. The corona pulses slowly and asymmetrically, its
> individual characters flickering and shifting between brightness levels like
> embers, while faint streamers drift outward and dissolve. The dark lunar disc
> at the centre stays perfectly still and perfectly black. Extremely subtle
> parallax: the whole character field drifts a few pixels, as if breathing. The
> serif wordmark and the thin rule beneath it remain completely static and
> sharp. Cinematic, patient, no camera movement, no zoom, no cuts. Loops
> seamlessly.

**Negative prompt**

> camera zoom, camera pan, dolly, fast motion, lens flare, sparkles, particles,
> smoke, photorealistic sun, fire, colour shift, rainbow, blue or purple tones,
> text distortion, warping letters, morphing typography, added text, watermark,
>人物, faces, hands

Notes for whoever drives this:

- **The wordmark must not move or warp.** Most image-to-video models will happily
  melt typography. If the model cannot hold it, generate the motion on a version
  of the still with no type and composite the wordmark back on afterwards — the
  clean plate is worth the extra step.
- **Keep the moon black.** A model that "helpfully" adds a glowing sun behind the
  disc has drawn a different thing entirely; that black circle is the product's
  whole metaphor.
- Aim for a low motion/strength setting. This should read as a long exposure that
  happens to be alive, not as an animation.
