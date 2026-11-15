/**
 * The cover chip standard (WP-9.4, docs/dev/card-corner-conventions.md):
 * every tone reads at 4.5:1 or better on the chip's backing, in the dark,
 * light and amber themes.
 *
 * Computed from `styles/globals.css` itself: the theme tokens, the
 * backing's opacity and the two mix amounts are read out of the file, so
 * retuning a status token or the chip without re-checking contrast fails
 * here. The backing is near-opaque, so a little of the cover shows
 * through; each tone is checked over the two extremes, pure black and
 * pure white art.
 */
import { readFileSync } from "node:fs";
import { resolve } from "node:path";

import { describe, expect, it } from "vitest";

const css = readFileSync(
  resolve(__dirname, "../../styles/globals.css"),
  "utf8",
);

type Lch = [number, number, number];
type Rgb = [number, number, number];

/** The declarations of the first rule whose selector text is `selector`. */
function rule(selector: string): string {
  const start = css.indexOf(`${selector} {`);
  expect(start, `rule ${selector}`).toBeGreaterThan(-1);
  const open = css.indexOf("{", start);
  // Declarations hold no braces, so the first `}` closes the rule (which
  // may sit inside an `@layer` block).
  return css.slice(open + 1, css.indexOf("}", open));
}

function oklchTokens(block: string): Record<string, Lch> {
  const out: Record<string, Lch> = {};
  for (const m of block.matchAll(/--([a-z-]+):\s*oklch\(([^)]+)\)/g)) {
    out[m[1]!] = m[2]!.trim().split(/\s+/).map(Number) as Lch;
  }
  return out;
}

const dark = oklchTokens(rule(':root,\n[data-theme="dark"]'));
const THEMES: Record<string, Record<string, Lch>> = {
  dark,
  light: { ...dark, ...oklchTokens(rule('\n[data-theme="light"]')) },
  amber: { ...dark, ...oklchTokens(rule('\n[data-theme="amber"]')) },
};

/** `--cover-chip-…: 94%` as a 0–1 fraction. */
function chipFraction(name: string): number {
  const m = new RegExp(`--cover-chip-${name}:\\s*([\\d.]+)%`).exec(css);
  expect(m, `--cover-chip-${name}`).toBeTruthy();
  return Number(m![1]) / 100;
}
const BACKING = chipFraction("backing");
const TONE = chipFraction("tone");
const SOLID = chipFraction("solid");

const toLab = ([l, c, h]: Lch): Lch => [
  l,
  c * Math.cos((h * Math.PI) / 180),
  c * Math.sin((h * Math.PI) / 180),
];

/** OKLab → gamma-encoded sRGB, clamped to the gamut. */
function labToRgb([L, a, b]: Lch): Rgb {
  const l = (L + 0.3963377774 * a + 0.2158037573 * b) ** 3;
  const m = (L - 0.1055613458 * a - 0.0638541728 * b) ** 3;
  const s = (L - 0.0894841775 * a - 1.291485548 * b) ** 3;
  const linear = [
    4.0767416621 * l - 3.3077115913 * m + 0.2309699292 * s,
    -1.2684380046 * l + 2.6097574011 * m - 0.3413193965 * s,
    -0.0041960863 * l - 0.7034186147 * m + 1.707614701 * s,
  ];
  return linear.map((x) => {
    const v = Math.min(1, Math.max(0, x));
    return v <= 0.0031308 ? 12.92 * v : 1.055 * v ** (1 / 2.4) - 0.055;
  }) as Rgb;
}

/** `color-mix(in oklab, a p, b)`. */
function mix(a: Lch, b: Lch, p: number): Rgb {
  const [x, y] = [toLab(a), toLab(b)];
  return labToRgb(x.map((v, i) => p * v + (1 - p) * y[i]!) as Lch);
}

function luminance(rgb: Rgb): number {
  const [r, g, b] = rgb.map((c) =>
    c <= 0.04045 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4,
  ) as Rgb;
  return 0.2126 * r + 0.7152 * g + 0.0722 * b;
}

function contrast(a: Rgb, b: Rgb): number {
  const [hi, lo] = [luminance(a), luminance(b)].sort((x, y) => y - x);
  return (hi! + 0.05) / (lo! + 0.05);
}

/** A translucent colour painted over an opaque one. */
const over = (top: Rgb, alpha: number, under: Rgb): Rgb =>
  top.map((c, i) => alpha * c + (1 - alpha) * under[i]!) as Rgb;

const BLACK: Rgb = [0, 0, 0];
const WHITE: Rgb = [1, 1, 1];
const AA = 4.5;

describe.each(Object.entries(THEMES))("cover chip in %s", (_name, t) => {
  const backing = labToRgb(toLab(t.background!));
  const onArt = [BLACK, WHITE].map((art) => over(backing, BACKING, art));
  /** Lowest contrast of `text` on the backing over either extreme. */
  const worst = (text: Rgb) =>
    Math.min(...onArt.map((bg) => contrast(text, bg)));

  it("neutral text clears 4.5:1 on the backing", () => {
    expect(worst(labToRgb(toLab(t.foreground!)))).toBeGreaterThanOrEqual(AA);
  });

  it.each(["success", "warning", "info"])(
    "%s text clears 4.5:1 on the backing",
    (tone) => {
      expect(worst(mix(t[tone]!, t.foreground!, TONE))).toBeGreaterThanOrEqual(
        AA,
      );
    },
  );

  it("the solid error chip's label clears 4.5:1 on its fill", () => {
    const fill = mix(t.destructive!, t.foreground!, SOLID);
    const label = labToRgb(toLab(t["destructive-foreground"]!));
    expect(contrast(label, fill)).toBeGreaterThanOrEqual(AA);
  });
});

describe("cover chip rules", () => {
  it("use the mixes this test measures", () => {
    for (const tone of ["success", "warning", "info"]) {
      const body = rule(`.cover-chip[data-tone="${tone}"]`).replace(
        /\s+/g,
        " ",
      );
      expect(body).toContain(
        `color-mix( in oklab, var(--${tone}) var(--cover-chip-tone), var(--foreground) )`,
      );
    }
    const base = rule(".cover-chip").replace(/\s+/g, " ");
    expect(base).toContain("color: var(--foreground)");
    expect(base).toContain(
      "color-mix( in oklab, var(--background) var(--cover-chip-backing), transparent )",
    );
    // Legible without the blur: the chip must not lean on it.
    expect(base).not.toContain("backdrop-filter");
    const error = rule('.cover-chip[data-tone="error"]').replace(/\s+/g, " ");
    expect(error).toContain("color: var(--destructive-foreground)");
    expect(error).toContain(
      "color-mix( in oklab, var(--destructive) var(--cover-chip-solid), var(--foreground) )",
    );
  });

  it("the raw status tokens would not pass, which is why they are mixed", () => {
    // Guards the premise: if a retune ever makes this pass, the mix can
    // be relaxed (and this expectation updated) rather than kept blindly.
    const t = THEMES.light!;
    const backing = over(labToRgb(toLab(t.background!)), BACKING, BLACK);
    expect(contrast(labToRgb(toLab(t.warning!)), backing)).toBeLessThan(AA);
  });
});
