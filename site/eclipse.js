/* The eclipse, drawn in characters — the same renderer the application ships.
 *
 * Its phase comes from how far down the page you are, so scrolling occults the
 * sun: the disc goes dark right about where the copy says passwords are never
 * written down. That is the only reason this page has any scroll effects at all.
 *
 * No dependencies, no build step. GitHub Pages serves three files.
 */
(() => {
  "use strict";

  document.documentElement.classList.add("js");

  const CELL_ASPECT = 0.5;
  const RAMP = ".,-~:;=!*#$@";
  const MOON_TO_SUN = 0.96;
  const OCCLUDES_TO = 1.22;
  const ADVANCE = 0.64;

  const reduced = window.matchMedia("(prefers-reduced-motion: reduce)");

  const moonOffset = (phase) => (phase - 0.5) * 2 * 2.4;

  function totality(phase) {
    const separation = Math.abs(moonOffset(phase));
    return Math.max(0, Math.min(1, 1 - separation / (1 + MOON_TO_SUN)));
  }

  function smoothstep(edge0, edge1, x) {
    const t = Math.max(0, Math.min(1, (x - edge0) / (edge1 - edge0)));
    return t * t * (3 - 2 * t);
  }

  function render(width, height, phase, time) {
    const cx = (width - 1) / 2;
    const cy = (height - 1) / 2;
    const radius = Math.min(width * CELL_ASPECT, height) * 0.29;
    const t = totality(phase);
    const moonDx = moonOffset(phase) * radius;
    const lines = [];

    for (let row = 0; row < height; row++) {
      let line = "";
      for (let col = 0; col < width; col++) {
        const x = (col - cx) * CELL_ASPECT;
        const y = row - cy;
        const fromSun = Math.hypot(x, y);
        const fromMoon = Math.hypot(x - moonDx, y);

        if (fromMoon <= radius * MOON_TO_SUN && fromSun <= radius * OCCLUDES_TO) {
          line += " ";
          continue;
        }
        if (fromSun <= radius) {
          line += "@";
          continue;
        }

        const out = (fromSun - radius) / radius;
        const angle = Math.atan2(y, x);
        const rays =
          0.5 + 0.5 * Math.sin(angle * 5 + time) * Math.sin(angle * 2.5 - time * 0.6);
        const falloff = Math.exp(-out * 2.2);
        // A bright collar on the limb; without it the disc edge dissolves into
        // speckle exactly where the eye is sharpest.
        const collar = (1 - smoothstep(0, 0.18, out)) * 0.85;
        const field = Math.max(collar, falloff * (0.4 + 0.6 * rays));
        const brightness = field * (0.34 + 0.66 * t);

        if (brightness <= 0.05) {
          line += " ";
          continue;
        }
        line += RAMP[Math.min(RAMP.length - 1, Math.floor(brightness * RAMP.length * 1.35))];
      }
      // Padded, not trimmed: every line is exactly `width`, so the block's box
      // is the grid and centring it centres the drawing.
      lines.push(line.padEnd(width, " "));
    }
    return lines.join("\n");
  }

  const pre = document.getElementById("eclipse");
  if (!pre) return;

  let cols = 60;
  let rows = 24;

  /* The grid is measured from the viewport rather than fixed, because this page
   * is read on everything from a phone to an ultrawide, and a fixed grid would
   * either overflow or float in the middle of the screen. */
  function fit() {
    const w = window.innerWidth;
    const h = window.innerHeight;
    const size = Math.max(7, Math.min(w / 46 / ADVANCE, h / 26));
    pre.style.fontSize = `${size}px`;
    cols = Math.max(24, Math.floor((w * 0.94) / (size * ADVANCE)));
    rows = Math.max(12, Math.floor((h * 0.94) / size));
  }

  /* Scroll drives the phase from a sliver of a crescent to well past totality.
   * It never starts at 0: a full bright disc on load reads as a blob, and the
   * first thing anyone sees should already look like an eclipse. */
  function phaseFromScroll() {
    const scrollable = document.body.scrollHeight - window.innerHeight;
    const progress = scrollable > 0 ? window.scrollY / scrollable : 0;
    return 0.16 + Math.max(0, Math.min(1, progress)) * 0.44;
  }

  let frame = 0;
  const started = performance.now();

  function draw(now) {
    // The corona breathes only when motion is welcome; the phase still tracks
    // scroll either way, because that is navigation rather than decoration.
    const time = reduced.matches ? 0.8 : (now - started) / 2600;
    pre.textContent = render(cols, rows, phaseFromScroll(), time);
    frame = reduced.matches ? 0 : requestAnimationFrame(draw);
  }

  function restart() {
    fit();
    if (frame) cancelAnimationFrame(frame);
    frame = requestAnimationFrame(draw);
  }

  fit();
  frame = requestAnimationFrame(draw);
  window.addEventListener("resize", restart);
  // Under reduced motion there is no rAF loop, so scrolling has to ask for a
  // frame itself.
  window.addEventListener("scroll", () => {
    if (reduced.matches) requestAnimationFrame(draw);
  });
  reduced.addEventListener("change", restart);

  /* Reveals. An observer rather than scroll maths: it fires once per element,
   * costs nothing while idle, and degrades to "everything visible" if the
   * browser lacks it. */
  const targets = document.querySelectorAll("[data-reveal]");
  if (!("IntersectionObserver" in window)) {
    targets.forEach((el) => el.classList.add("in"));
    return;
  }

  const observer = new IntersectionObserver(
    (entries) => {
      for (const entry of entries) {
        if (!entry.isIntersecting) continue;
        entry.target.classList.add("in");
        observer.unobserve(entry.target);
      }
    },
    { rootMargin: "0px 0px -12% 0px", threshold: 0.15 },
  );
  targets.forEach((el) => observer.observe(el));
})();
