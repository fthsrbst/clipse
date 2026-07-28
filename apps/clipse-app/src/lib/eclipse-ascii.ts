/**
 * The eclipse, drawn in characters.
 *
 * This is the one picture Clipse has of itself, and it is not a picture: it is
 * computed per frame from a phase between 0 and 1, so the onboarding can move
 * the moon across the sun as the reader moves down the page instead of cutting
 * between stills. Totality lands where the product's promise does — the moment
 * the disc goes dark is the moment the copy says nothing leaves your machine.
 *
 * Everything here is pure. `render` takes a phase and gives back lines; the
 * animating is somebody else's problem.
 */

/** Character cells are about twice as tall as they are wide. Circles drawn
 * without correcting for that come out as eggs. */
const CELL_ASPECT = 0.5;

/** Dim to bright. Space is not in the ramp: the background is drawn from the
 * corona field, and an empty cell has to stay genuinely empty. */
const RAMP = ".,-~:;=!*#$@";

/** Sun and moon are nearly the same size in the sky, which is the whole reason
 * a total eclipse looks the way it does — and why the corona is visible at all.
 */
const MOON_TO_SUN = 0.96;

/**
 * How far out the moon still blocks light, in sun radii.
 *
 * The moon is not drawn as an object: it has no light of its own, so away from
 * the sun it is simply the sky. Occluding only near the disc keeps it from
 * carving a large empty circle out of the corona, which reads as a rendering
 * fault rather than as an eclipse.
 */
const OCCLUDES_TO = 1.22;

function smoothstep(edge0: number, edge1: number, x: number): number {
  const t = Math.max(0, Math.min(1, (x - edge0) / (edge1 - edge0)));
  return t * t * (3 - 2 * t);
}

export interface EclipseOptions {
  width: number;
  height: number;
  /** 0 = moon clear of the sun, 0.5 = totality, 1 = clear on the other side. */
  phase: number;
  /** Radians, drifts the corona so successive frames are not identical. */
  time?: number;
}

/**
 * How far the moon's centre sits from the sun's, in sun radii.
 *
 * Linear in phase: the moon crosses at a constant rate, as it does. The travel
 * is 2.4 radii each way so the disc starts and ends fully clear of the corona
 * rather than sitting half in it.
 */
function moonOffset(phase: number): number {
  return (phase - 0.5) * 2 * 2.4;
}

/** 1 at totality, 0 once the discs are clear of each other. Drives how much
 * corona is visible — in daylight you cannot see it at all. */
function totality(phase: number): number {
  const separation = Math.abs(moonOffset(phase));
  const t = 1 - separation / (1 + MOON_TO_SUN);
  return Math.max(0, Math.min(1, t));
}

export function render({ width, height, phase, time = 0 }: EclipseOptions): string[] {
  const cx = (width - 1) / 2;
  const cy = (height - 1) / 2;
  // Leave room for the corona to breathe; the disc is deliberately small
  // relative to the panel.
  const radius = Math.min(width * CELL_ASPECT, height) * 0.29;

  const t = totality(phase);
  const moonDx = moonOffset(phase) * radius;
  const lines: string[] = [];

  for (let row = 0; row < height; row++) {
    let line = "";
    for (let col = 0; col < width; col++) {
      // Into a space where x and y are the same physical size.
      const x = (col - cx) * CELL_ASPECT;
      const y = row - cy;

      const fromSun = Math.hypot(x, y);
      const fromMoon = Math.hypot(x - moonDx, y);

      if (fromMoon <= radius * MOON_TO_SUN && fromSun <= radius * OCCLUDES_TO) {
        // The moon is not dark against the sky, it is *the* dark thing.
        line += " ";
        continue;
      }

      if (fromSun <= radius) {
        line += "@";
        continue;
      }

      // Corona: falls off with distance, breathes with time, and is only
      // really there when the disc is covered.
      const out = (fromSun - radius) / radius;
      const angle = Math.atan2(y, x);
      const rays = 0.5 + 0.5 * Math.sin(angle * 5 + time) * Math.sin(angle * 2.5 - time * 0.6);
      const falloff = Math.exp(-out * 2.2);
      // A bright collar hugging the limb. Without it the disc edge dissolves
      // into speckle exactly where the eye is sharpest, and the sun stops
      // reading as a solid body.
      const collar = (1 - smoothstep(0, 0.18, out)) * 0.85;
      const field = Math.max(collar, falloff * (0.4 + 0.6 * rays));
      const brightness = field * (0.34 + 0.66 * t);

      if (brightness <= 0.05) {
        line += " ";
        continue;
      }
      const index = Math.min(RAMP.length - 1, Math.floor(brightness * RAMP.length * 1.35));
      line += RAMP[index];
    }
    // Padded rather than trimmed. Trailing spaces are invisible, but they make
    // every line exactly `width` characters, so the block's box is the grid and
    // centring it centres the drawing. Trimmed lines make the box as wide as
    // the widest row, which drifts as the moon moves.
    lines.push(line.padEnd(width, " "));
  }

  return lines;
}

/**
 * The mark, at a size that fits beside a heading.
 *
 * Fixed rather than computed: the logotype has to be identical everywhere it
 * appears, including in places that never animate.
 */
export const ECLIPSE_MARK = [
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
